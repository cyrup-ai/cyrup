//! The Together provider (arch-01 §5). Speaks the [`openai-completions`](crate::api::openai_completions)
//! wire protocol; differs from other OpenAI-compatible providers only in base URL, env key, and
//! catalog. Mirrors Pi's `providers/together.ts` + `together.models.ts`.

use crate::api::{builtin_registry, ApiRegistry};
use crate::auth::{env_key, CredentialStore, InMemoryCredentialStore, ProviderAuth};
use crate::known_api::OPENAI_COMPLETIONS;
use crate::model::{Modality, Model, ModelCost};
use crate::wire::WireProvider;
use std::sync::Arc;

/// Together's OpenAI-compatible base URL (the `/chat/completions` path is appended by the wire impl).
pub const TOGETHER_BASE_URL: &str = "https://api.together.ai/v1";

/// The provider id.
pub const TOGETHER_PROVIDER_ID: &str = "together";

/// The env var carrying the Together API key (func-01 R-01-063).
pub const TOGETHER_API_KEY_ENV: &str = "TOGETHER_API_KEY";

/// Build one Together catalog entry.
fn model(
    id: &str,
    name: &str,
    reasoning: bool,
    image: bool,
    cost: ModelCost,
    context_window: u64,
    max_tokens: u64,
) -> Model {
    let mut input = vec![Modality::Text];
    if image {
        input.push(Modality::Image);
    }
    Model {
        id: id.into(),
        name: name.into(),
        api: OPENAI_COMPLETIONS.into(),
        provider: TOGETHER_PROVIDER_ID.into(),
        base_url: Some(TOGETHER_BASE_URL.to_string()),
        reasoning,
        input,
        output: vec![Modality::Text],
        cost,
        context_window,
        max_tokens,
    }
}

/// A hand-seeded subset of current Together chat models (id/name/context/cost from Pi's
/// `together.models.ts`). Full models.dev generation is DEFERRED (arch-01 §12).
pub fn together_models() -> Vec<Model> {
    let c = |input, output, cache_read| ModelCost { input, output, cache_read, cache_write: 0.0 };
    vec![
        model("openai/gpt-oss-120b", "GPT OSS 120B", true, false, c(0.15, 0.6, 0.0), 131_072, 131_072),
        model("openai/gpt-oss-20b", "GPT OSS 20B", true, false, c(0.05, 0.2, 0.0), 131_072, 131_072),
        model(
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            "Llama 3.3 70B",
            false,
            false,
            c(0.88, 0.88, 0.0),
            131_072,
            131_072,
        ),
        model(
            "Qwen/Qwen2.5-7B-Instruct-Turbo",
            "Qwen 2.5 7B Instruct Turbo",
            false,
            false,
            c(0.3, 0.3, 0.0),
            32_768,
            32_768,
        ),
        model(
            "deepseek-ai/DeepSeek-V4-Pro",
            "DeepSeek V4 Pro",
            true,
            false,
            c(1.74, 3.48, 0.2),
            512_000,
            384_000,
        ),
        model(
            "moonshotai/Kimi-K2.6",
            "Kimi K2.6",
            true,
            true,
            c(1.2, 4.5, 0.2),
            262_144,
            131_000,
        ),
        model("zai-org/GLM-5", "GLM-5", true, false, c(1.0, 3.2, 0.0), 202_752, 131_072),
    ]
}

/// The Together [`ProviderAuth`]: an API key from `$TOGETHER_API_KEY` (R-01-063).
pub fn together_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key([TOGETHER_API_KEY_ENV]))
}

/// Construct the Together provider over the given credential store + shared api registry. The
/// registry MUST provide the `openai-completions` impl (use [`builtin_registry`]).
pub fn together_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        TOGETHER_PROVIDER_ID,
        "Together",
        together_models(),
        together_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry. Resolves
/// auth from `$TOGETHER_API_KEY` (or a stored credential / per-request override). Suitable for
/// `bin/session-svc` and demos.
pub fn together_provider() -> WireProvider {
    together_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::auth::types::AuthContext;
    use crate::context::Context;
    use crate::provider::Provider;
    use crate::stream::{collect_message, StreamOptions};
    use cyrup_core::StopReason;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// Map-backed env context for deterministic auth tests.
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
    fn exposes_catalog_of_openai_completions_models() {
        let p = together_provider();
        assert_eq!(p.id().as_str(), "together");
        let models = p.models();
        assert!(models.len() >= 5);
        assert!(models.iter().all(|m| m.api.as_str() == OPENAI_COMPLETIONS));
        assert!(models.iter().all(|m| m.provider.as_str() == "together"));
        assert!(models.iter().all(|m| m.base_url.as_deref() == Some(TOGETHER_BASE_URL)));
        assert!(p.get_model("openai/gpt-oss-120b").is_some());
    }

    #[tokio::test]
    async fn unconfigured_without_env_yields_error_terminal() {
        // Empty env + empty store → not configured → terminal Error (never a panic).
        let provider = together_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(MapEnv(BTreeMap::new())));
        let model = provider.get_model("openai/gpt-oss-120b").unwrap().clone();
        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.unwrap().contains("not configured"));
    }

    #[tokio::test]
    async fn resolves_auth_from_together_api_key_env() {
        // A bogus base URL makes the request fail at transport AFTER auth resolves — proving the
        // env key resolved (an unconfigured provider would short-circuit with "not configured").
        let env = MapEnv(BTreeMap::from([(
            TOGETHER_API_KEY_ENV.to_string(),
            "sk-together-test".to_string(),
        )]));
        let provider = together_provider_with(
            Arc::new(InMemoryCredentialStore::new()),
            Arc::new(builtin_registry()),
        )
        .with_auth_context(Arc::new(env));

        let mut model = provider.get_model("openai/gpt-oss-120b").unwrap().clone();
        // Point at an unroutable address so connect fails fast.
        model.base_url = Some("http://127.0.0.1:1/v1".to_string());

        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        // Auth resolved (no "not configured"); failure is a transport error terminal.
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(!err.contains("not configured"), "auth should have resolved, got: {err}");
        assert!(err.contains("transport"), "expected transport error, got: {err}");
    }

    /// Live smoke test against the real Together API. Ignored by default; run with
    /// `TOGETHER_API_KEY` set: `cargo test -p cyrup-provider -- --ignored live_together`.
    #[tokio::test]
    #[ignore = "hits the real Together API; requires TOGETHER_API_KEY"]
    async fn live_together_returns_non_empty_done() {
        use cyrup_core::{Content, Message};

        if std::env::var("TOGETHER_API_KEY").is_err() {
            eprintln!("skipping: TOGETHER_API_KEY not set");
            return;
        }

        // Real env-backed provider (default EnvAuthContext reads the process env).
        let provider = together_provider();
        let model = provider.get_model("openai/gpt-oss-120b").unwrap().clone();
        let ctx = Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![Content::text("Reply with exactly: pong")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        // Enough budget for a real answer. gpt-oss-120b is a reasoning model: it may finish with
        // `Stop` (answer emitted) or, if it spends the budget reasoning, `Length` — both are
        // successful round-trips. Only a transport/API failure yields `Error`.
        let opts = StreamOptions { max_tokens: Some(256), ..Default::default() };
        let msg = collect_message(provider.stream(&model, &ctx, &opts)).await;

        assert_ne!(
            msg.stop_reason,
            StopReason::Error,
            "expected a successful terminal, got error: {:?}",
            msg.error_message
        );
        // Decoding produced real content — assistant text and/or reasoning.
        let has_content = msg.content.iter().any(|c| match c {
            Content::Text { text, .. } => !text.trim().is_empty(),
            Content::Thinking { thinking, .. } => !thinking.trim().is_empty(),
            _ => false,
        });
        assert!(has_content, "expected non-empty assistant content, got: {:?}", msg.content);
    }
}
