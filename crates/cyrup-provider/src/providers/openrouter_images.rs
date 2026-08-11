//! The OpenRouter image-generation provider (1:1 port of Pi `providers/openrouter-images.ts`).
//!
//! Speaks the [`openrouter-images`](crate::images::openrouter) wire protocol. Auth:
//! `OPENROUTER_API_KEY` (Pi `envApiKeyAuth("OpenRouter API key", ["OPENROUTER_API_KEY"])`). Its
//! catalog is the verbatim generated `IMAGE_MODELS.openrouter` (35 models).

use crate::auth::{ProviderAuth, env_key};
use crate::images::{
    BuiltImagesProvider, CreateImagesProviderOptions, OPENROUTER_PROVIDER_ID,
    create_images_provider, openrouter, openrouter_image_models,
};

/// The OpenRouter image provider's [`ProviderAuth`]: `OPENROUTER_API_KEY` (Pi `envApiKeyAuth`)
/// plus the OpenRouter OAuth login (`lazyOAuth({ name: "OpenRouter OAuth", loginLabel: "Sign in
/// with OpenRouter", load: loadOpenRouterOAuth })`, `providers/openrouter-images.ts:13-17`) — no
/// `isSubscription`, because OpenRouter bills per token. See
/// [`super::builtin_oauth::builtin_provider_oauth`].
pub fn openrouter_images_auth() -> ProviderAuth {
    ProviderAuth {
        api_key: Some(env_key(["OPENROUTER_API_KEY"])),
        oauth: super::builtin_oauth::builtin_provider_oauth(OPENROUTER_PROVIDER_ID),
    }
}

/// Construct the OpenRouter image provider (Pi `openrouterImagesProvider`, openrouter-images.ts:6).
pub fn openrouter_images_provider() -> BuiltImagesProvider {
    create_images_provider(CreateImagesProviderOptions {
        id: OPENROUTER_PROVIDER_ID.to_string(),
        name: Some("OpenRouter".to_string()),
        auth: openrouter_images_auth(),
        models: openrouter_image_models(),
        refresh_models: None,
        api: openrouter::factory(),
    })
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
    use crate::auth::types::AuthContext;
    use crate::collection::CreateModelsOptions;
    use crate::images::{ImagesContext, ImagesOptions, ImagesStopReason, create_images_models};
    use cyrup_core::Content;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct MapEnv(BTreeMap<String, String>);
    #[async_trait::async_trait]
    impl AuthContext for MapEnv {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    #[test]
    fn provider_identity_and_catalog() {
        use crate::images::ImagesProvider;
        let p = openrouter_images_provider();
        assert_eq!(p.id(), "openrouter");
        assert_eq!(p.name(), "OpenRouter");
        assert_eq!(p.get_models().len(), 35);
        assert!(p.provider_auth().is_some());
    }

    #[tokio::test]
    async fn unconfigured_without_env_yields_no_api_key_error() {
        // No env key configured → the collection delegates without auth, and the wire impl returns the
        // "No API key" error envelope (Pi openrouter-images.ts:53-56).
        let mut models = create_images_models(CreateModelsOptions {
            credentials: None,
            auth_context: Some(Arc::new(MapEnv(BTreeMap::new()))),
            catalog_overlay: None,
        });
        models.set_provider(Arc::new(openrouter_images_provider()));
        let m = models
            .get_model("openrouter", "google/gemini-2.5-flash-image")
            .expect("model");
        let ctx = ImagesContext {
            input: vec![Content::text("a red square")],
        };
        let out = models
            .generate_images(&m, &ctx, &ImagesOptions::default())
            .await;
        assert_eq!(out.stop_reason, ImagesStopReason::Error);
        assert!(out.error_message.unwrap().contains("No API key"));
    }
}
