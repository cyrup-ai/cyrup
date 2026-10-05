//! The OpenRouter provider's non-chat legs (pi `packages/ai/src/providers/openrouter.ts` @v1.0.1).
//!
//! PROV-128. Upstream's `openrouterProvider()` is ONE `createProvider` call whose `models` array
//! holds chat, image and classifier rows together and whose operation maps name an implementation
//! per api:
//!
//! ```text
//! models: [...Object.values(OPENROUTER_MODELS),
//!          ...Object.values(OPENROUTER_IMAGE_MODELS),
//!          ...Object.values(OPENROUTER_CLASSIFIER_MODELS)],
//! api:         { "anthropic-messages": …, "openai-completions": … },
//! images:      { "openrouter-images": openrouterImagesApi() },   // :33
//! classifiers: { "typesafe-system-one": typesafeSystemOneApi() }, // :35
//! ```
//!
//! cyrup compresses the twenty single-file `createProvider` factories that are purely
//! `(id, name, env var, catalog, one wire protocol)` into [`super::fleet`]'s table, and keeps the
//! per-provider exceptions beside it — the same way [`super::builtin_oauth::builtin_provider_oauth`]
//! keeps `openrouter`'s and `xai`'s `lazyOAuth` clause outside the table. The image leg is such an
//! exception: `openrouter` is the only built-in with one. [`with_builtin_images`] is the hook
//! [`super::fleet::FleetSpec::provider_with`] calls, so the fleet-built `openrouter` provider comes
//! out of `builtin_providers_with` already carrying its image rows and its `images` map.
//!
//! Not ported: `classifiers: { "typesafe-system-one": … }` (`:35`). `PROV-104` owns the two System
//! One classifier apis, and `KnownClassifierApi` names only `llama-cpp-classify`
//! ([`crate::classifier::KnownClassifierApi`]). The live catalog's ten `openrouter` classifier rows
//! therefore parse, persist and list as [`crate::AnyModel::Classifier`] — which is what PROV-128
//! is responsible for — and `classify()` on one answers
//! `Provider openrouter has no classifier implementation for "typesafe-system-one"`, pi's own
//! message for a model whose api has no entry (`models.ts:1168`).

use crate::classifier::ImageModel;
use crate::images::ImageApiRegistry;
use crate::wire::WireProvider;
use std::sync::Arc;

/// The openrouter image catalog, extracted verbatim from pi's generated image rows (PROV-089 /
/// PROV-129: `IMAGE_MODELS.openrouter`, pinned at v0.87.1 because `image-models.generated.ts` was
/// deleted at v1.0.0 and its successor `models.generated.ts` is gitignored).
const OPENROUTER_IMAGES_CATALOG_JSON: &str = include_str!("catalog/openrouter-images.json");

/// The full openrouter image catalog (1:1 with pi `OPENROUTER_IMAGE_MODELS`). A parse failure
/// yields an empty catalog (surfaced by the catalog-count test in [`crate::tests::catalog_data`])
/// rather than a panic.
///
/// **The `type` discriminant is supplied here, not by the file.** The embedded rows were extracted
/// at v0.87.1, where an image row was an `ImagesModel` in a registry of its own and carried no
/// `type` member — the whole FILE was the discriminant. v1.0.0's [`ImageModel`] makes
/// `type: "image"` required on read, deliberately, so a chat row's JSON cannot be read as an image
/// row inside one mixed array ([`crate::AnyModel`]'s deserializer dispatches on it). Stamping it
/// per row while loading is what turns the one into the other; it is not a widening, because this
/// file's rows are image rows by construction and the live endpoint's are stamped already (checked
/// 2026-10-05: every `?types=` image row carries `"type":"image"`).
pub fn openrouter_image_models() -> Vec<ImageModel> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(OPENROUTER_IMAGES_CATALOG_JSON)
    else {
        return Vec::new();
    };
    rows.into_iter()
        .filter_map(|mut row| {
            let object = row.as_object_mut()?;
            object.insert(
                "type".to_string(),
                serde_json::Value::String(crate::classifier::ModelType::Image.as_str().to_string()),
            );
            serde_json::from_value::<ImageModel>(row).ok()
        })
        .collect()
}

/// One openrouter image model by id (pi `getBuiltinImageModel("openrouter", id)`,
/// `providers/all.ts:72-79`).
pub fn openrouter_image_model(model_id: &str) -> Option<ImageModel> {
    openrouter_image_models()
        .into_iter()
        .find(|m| m.id.as_str() == model_id)
}

/// The provider's `images` dispatch map (pi `images: { "openrouter-images": openrouterImagesApi() }`,
/// `providers/openrouter.ts:33`).
pub fn openrouter_images_registry() -> ImageApiRegistry {
    let mut registry = ImageApiRegistry::new();
    registry.register(
        crate::images::KnownImageApi::OpenrouterImages,
        crate::api::openrouter_images::openrouter_images_api(),
    );
    registry
}

/// Attach the built-in image leg to the provider `id`, if it has one — the per-id hook
/// [`super::fleet::FleetSpec::provider_with`] applies, shaped exactly like
/// [`super::builtin_oauth::builtin_provider_oauth`].
///
/// `openrouter` is the only built-in with image models upstream, so every other id is returned
/// UNCHANGED and cannot acquire `generate_images` by accident
/// ([`crate::provider::Provider::supports_image_generation`] stays false for it).
pub fn with_builtin_images(id: &str, provider: WireProvider) -> WireProvider {
    if id != super::fleet::OPENROUTER.id {
        return provider;
    }
    provider
        .with_image_models(openrouter_image_models())
        .with_images(Arc::new(openrouter_images_registry()))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::classifier::AnyModel;
    use crate::images::{ImagesContext, ImagesOptions, ImagesStopReason, OPENROUTER_IMAGES};
    use crate::model::Modality;
    use crate::provider::Provider;
    use crate::providers::fleet::OPENROUTER;

    /// The v0.87.1 extract, read through v1.0.0's `ImageModel` shape. The count is the same 55 the
    /// deleted `crate::images::openrouter_image_models` asserted, which is the evidence that
    /// stamping `type` loses no row.
    #[test]
    fn the_catalog_parses_verbatim_as_image_models() {
        let models = openrouter_image_models();
        assert_eq!(models.len(), 55);
        assert!(models.iter().all(|m| m.api.as_str() == OPENROUTER_IMAGES));
        assert!(models.iter().all(|m| m.provider.as_str() == "openrouter"));
        assert!(
            models
                .iter()
                .all(|m| m.base_url == "https://openrouter.ai/api/v1")
        );
        // Every image model emits image output (pi `ImageModel.output` always includes "image").
        assert!(models.iter().all(|m| m.output.contains(&Modality::Image)));
        // Nano Banana emits both text + image; FLUX is image-only.
        assert!(
            openrouter_image_model("google/gemini-2.5-flash-image")
                .expect("nano banana")
                .outputs_text()
        );
        assert!(
            !openrouter_image_model("black-forest-labs/flux.2-flex")
                .expect("flux")
                .outputs_text()
        );
        assert!(openrouter_image_model("nope").is_none());
    }

    /// PROV-128 steps (2)+(3) end to end on the built-in provider: the fleet-built `openrouter`
    /// lists its image rows through `get_all_models()` as [`AnyModel::Image`], and
    /// `supports_image_generation()` is true because the provider was built with an `images` map
    /// (pi attaches `generateImages` exactly then, `models.ts:1146`).
    #[test]
    fn the_builtin_openrouter_provider_lists_image_models_and_supports_generation() {
        let provider = OPENROUTER.provider();
        let all = provider.get_all_models();
        let images: Vec<_> = all.iter().filter_map(AnyModel::as_image).collect();
        assert_eq!(images.len(), 55, "openrouter must list its image rows");
        assert!(
            all.iter().filter_map(AnyModel::as_chat).count() >= 300,
            "the chat rows must still be there"
        );
        assert!(provider.supports_image_generation());
        // The chat catalog is UNCHANGED: `models()` is chat-only and cannot hand out an image row.
        assert!(
            provider
                .models()
                .iter()
                .all(|m| m.api.as_str() != OPENROUTER_IMAGES)
        );
    }

    /// Every other fleet member is left alone: no image rows, no `generate_images`.
    #[test]
    fn no_other_builtin_acquires_an_image_leg() {
        for spec in crate::providers::fleet::FLEET {
            if spec.id == OPENROUTER.id {
                continue;
            }
            let provider = spec.provider();
            assert!(
                !provider.supports_image_generation(),
                "{} must not support image generation",
                spec.id
            );
            assert!(
                provider
                    .get_all_models()
                    .iter()
                    .all(|m| m.as_image().is_none()),
                "{} must list no image rows",
                spec.id
            );
        }
    }

    /// **Support is checked BEFORE auth** (pi `models.ts:954-959`: the `!provider.generateImages`
    /// throw precedes `applyAuth`). A provider that lists image rows but was built with no `images`
    /// map must be answered without a credential read — so the message is "does not support image
    /// generation", NEVER the auth stage's "Provider is not configured".
    ///
    /// The two messages are what makes the ordering observable: the provider here is unconfigured,
    /// so if the support check were skipped the request would reach `applyAuth` and say so.
    #[tokio::test]
    async fn image_support_is_checked_before_auth() {
        use crate::auth::{AuthContext, InMemoryCredentialStore, ProviderAuth, env_key};
        use crate::collection::{CreateModelsOptions, create_models};

        /// An environment with nothing in it, so the provider is unconfigured.
        struct NoEnv;
        #[async_trait::async_trait]
        impl AuthContext for NoEnv {
            async fn env(&self, _name: &str) -> Option<String> {
                None
            }
            async fn file_exists(&self, _path: &str) -> bool {
                false
            }
        }

        // Image rows, deliberately NO `with_images(..)`: pi attaches `generateImages` only when the
        // `images` option carries an implementation (`models.ts:1146`).
        let provider = crate::wire::WireProvider::new(
            "imageless",
            "Imageless",
            Vec::new(),
            ProviderAuth::with_api_key(env_key("Imageless key", ["IMAGELESS_API_KEY"])),
            std::sync::Arc::new(InMemoryCredentialStore::new()),
            std::sync::Arc::new(crate::api::builtin_registry()),
        )
        .with_image_models(openrouter_image_models());
        assert!(
            !provider.supports_image_generation(),
            "image rows alone must not imply support"
        );

        let mut models = create_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(std::sync::Arc::new(NoEnv)),
            catalog_overlay: None,
        });
        models.set_provider(std::sync::Arc::new(provider));
        let mut model =
            openrouter_image_model("google/gemini-2.5-flash-image").expect("nano banana");
        model.provider = "imageless".into();

        let out = models
            .generate_images(&model, &ImagesContext::default(), &ImagesOptions::default())
            .await;
        assert_eq!(out.stop_reason, ImagesStopReason::Error);
        let message = out.error_message.expect("a message");
        assert_eq!(
            message, "Provider imageless does not support image generation",
            "support must be answered before auth"
        );
        assert!(
            !message.contains("not configured"),
            "a provider that cannot generate images must not need a credential: {message}"
        );
    }

    /// A provider with no image leg answers pi's absent-member message rather than silently
    /// succeeding (`models.ts:956-959`, via [`Provider::generate_images`]'s default).
    #[tokio::test]
    async fn a_provider_without_images_reports_it() {
        let provider = crate::providers::fleet::GROQ.provider();
        let mut model =
            openrouter_image_model("google/gemini-2.5-flash-image").expect("nano banana");
        model.provider = "groq".into();
        let out = provider
            .generate_images(&model, &ImagesContext::default(), &ImagesOptions::default())
            .await;
        assert_eq!(out.stop_reason, ImagesStopReason::Error);
        assert_eq!(
            out.error_message.as_deref(),
            Some("Provider groq does not support image generation")
        );
    }
}
