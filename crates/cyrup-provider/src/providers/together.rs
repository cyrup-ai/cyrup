//! The Together provider (arch-01 §5). Speaks the [`openai-completions`](crate::api::openai_completions)
//! wire protocol; differs from other OpenAI-compatible providers only in base URL, env key, and
//! catalog. Mirrors Pi's `providers/together.ts` + `together.models.ts`.

use crate::api::compat::{MaxTokensField, OpenAiCompletionsCompat, ThinkingFormat};
use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::{CredentialStore, InMemoryCredentialStore, ProviderAuth, env_key};
use crate::known_api::OPENAI_COMPLETIONS;
use crate::model::{Modality, Model, ModelCost, ThinkingLevelMap};
use crate::wire::WireProvider;
use std::sync::Arc;

/// Together's OpenAI-compatible base URL (the `/chat/completions` path is appended by the wire impl).
pub const TOGETHER_BASE_URL: &str = "https://api.together.ai/v1";

/// The provider id.
pub const TOGETHER_PROVIDER_ID: &str = "together";

/// The env var carrying the Together API key (func-01 R-01-063).
pub const TOGETHER_API_KEY_ENV: &str = "TOGETHER_API_KEY";

/// USD-per-1e6-token cost (Together never reports a separate cache-write price).
fn cost(input: f64, output: f64, cache_read: f64) -> ModelCost {
    ModelCost {
        input,
        output,
        cache_read,
        cache_write: 0.0,
        tiers: None,
    }
}

/// Build a `thinkingLevelMap` from `(level, Some(value) | None)` pairs (Pi `ThinkingLevelMap`).
fn level_map(pairs: &[(&str, Option<&str>)]) -> ThinkingLevelMap {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.map(|s| s.to_string())))
        .collect()
}

/// The Together compat block shared by every catalog entry (Pi `together.models.ts` `compat`):
/// no `store`/`developer`/`strict`/long-cache, `max_tokens` field. The two varying knobs are
/// `supports_reasoning_effort` and `thinking_format`.
fn together_compat(
    supports_reasoning_effort: bool,
    thinking_format: Option<ThinkingFormat>,
) -> OpenAiCompletionsCompat {
    OpenAiCompletionsCompat {
        supports_store: Some(false),
        supports_developer_role: Some(false),
        supports_reasoning_effort: Some(supports_reasoning_effort),
        max_tokens_field: Some(MaxTokensField::MaxTokens),
        thinking_format,
        supports_strict_mode: Some(false),
        supports_long_cache_retention: Some(false),
        ..Default::default()
    }
}

/// Build one Together catalog entry.
#[allow(clippy::too_many_arguments)]
fn model(
    id: &str,
    name: &str,
    reasoning: bool,
    image: bool,
    cost: ModelCost,
    context_window: u64,
    max_tokens: u64,
    thinking_level_map: Option<ThinkingLevelMap>,
    compat: OpenAiCompletionsCompat,
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
        base_url: TOGETHER_BASE_URL.to_string(),
        reasoning,
        input,
        cost,
        context_window,
        max_tokens,
        thinking_level_map,
        compat: Some(compat),
        headers: None,
    }
}

/// The full Together chat catalog — a verbatim 1:1 port of Pi's `together.models.ts`
/// (every model id, name, cost, context window, max tokens, reasoning flag, modalities, the
/// per-model `compat` block, and `thinkingLevelMap`).
pub fn together_models() -> Vec<Model> {
    // The default reasoning map shared by most Together reasoning models.
    let m = || level_map(&[("minimal", None), ("low", None), ("medium", None)]);
    vec![
        model(
            "MiniMaxAI/MiniMax-M2.7",
            "MiniMax-M2.7",
            true,
            false,
            cost(0.3, 1.2, 0.06),
            202_752,
            131_072,
            Some(level_map(&[
                ("off", None),
                ("minimal", None),
                ("low", None),
                ("medium", None),
            ])),
            // No `thinkingFormat` override => falls back to detected "together".
            together_compat(false, None),
        ),
        model(
            "MiniMaxAI/MiniMax-M3",
            "MiniMax-M3",
            true,
            true,
            cost(0.3, 1.2, 0.06),
            524_288,
            250_000,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "Qwen/Qwen2.5-7B-Instruct-Turbo",
            "Qwen 2.5 7B Instruct Turbo",
            false,
            false,
            cost(0.3, 0.3, 0.0),
            32_768,
            32_768,
            None,
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "Qwen/Qwen3-235B-A22B-Instruct-2507-tput",
            "Qwen3 235B A22B Instruct 2507 FP8",
            false,
            false,
            cost(0.2, 0.6, 0.0),
            262_144,
            262_144,
            None,
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "Qwen/Qwen3.5-397B-A17B",
            "Qwen3.5 397B A17B",
            true,
            true,
            cost(0.6, 3.6, 0.0),
            262_144,
            130_000,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "Qwen/Qwen3.5-9B",
            "Qwen3.5 9B",
            true,
            true,
            cost(0.17, 0.25, 0.0),
            262_144,
            65_536,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "Qwen/Qwen3.6-Plus",
            "Qwen3.6 Plus",
            true,
            false,
            cost(0.5, 3.0, 0.0),
            1_000_000,
            500_000,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "Qwen/Qwen3.7-Max",
            "Qwen3.7 Max",
            false,
            false,
            cost(1.25, 3.75, 0.0),
            1_000_000,
            500_000,
            None,
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "deepseek-ai/DeepSeek-V4-Pro",
            "DeepSeek V4 Pro",
            true,
            false,
            cost(1.74, 3.48, 0.2),
            512_000,
            384_000,
            Some(level_map(&[
                ("minimal", None),
                ("low", None),
                ("medium", None),
                ("high", Some("high")),
                ("xhigh", None),
            ])),
            together_compat(true, Some(ThinkingFormat::Together)),
        ),
        model(
            "essentialai/Rnj-1-Instruct",
            "Rnj-1 Instruct",
            false,
            false,
            cost(0.15, 0.15, 0.0),
            32_768,
            32_768,
            None,
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "google/gemma-4-31B-it",
            "Gemma 4 31B Instruct",
            true,
            true,
            cost(0.39, 0.97, 0.0),
            262_144,
            131_072,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            "Llama 3.3 70B",
            false,
            false,
            cost(0.88, 0.88, 0.0),
            131_072,
            131_072,
            None,
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "moonshotai/Kimi-K2.6",
            "Kimi K2.6",
            true,
            true,
            cost(1.2, 4.5, 0.2),
            262_144,
            131_000,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "moonshotai/Kimi-K2.7-Code",
            "Kimi K2.7 Code",
            true,
            false,
            cost(0.95, 4.0, 0.19),
            262_144,
            131_072,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "nvidia/nemotron-3-ultra-550b-a55b",
            "Nemotron 3 Ultra 550B A55B",
            true,
            false,
            cost(0.6, 3.6, 0.2),
            512_300,
            512_300,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "openai/gpt-oss-120b",
            "GPT OSS 120B",
            true,
            false,
            cost(0.15, 0.6, 0.0),
            131_072,
            131_072,
            Some(level_map(&[("off", None), ("minimal", None)])),
            together_compat(true, Some(ThinkingFormat::Openai)),
        ),
        model(
            "openai/gpt-oss-20b",
            "GPT OSS 20B",
            true,
            false,
            cost(0.05, 0.2, 0.0),
            131_072,
            131_072,
            Some(level_map(&[("off", None), ("minimal", None)])),
            together_compat(true, Some(ThinkingFormat::Openai)),
        ),
        model(
            "zai-org/GLM-5",
            "GLM-5",
            true,
            false,
            cost(1.0, 3.2, 0.0),
            202_752,
            131_072,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "zai-org/GLM-5.1",
            "GLM-5.1",
            true,
            false,
            cost(1.4, 4.4, 0.0),
            202_752,
            131_072,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
        model(
            "zai-org/GLM-5.2",
            "GLM-5.2",
            true,
            false,
            cost(1.4, 4.4, 0.26),
            262_144,
            164_000,
            Some(m()),
            together_compat(false, Some(ThinkingFormat::Together)),
        ),
    ]
}

/// The Together [`ProviderAuth`]: an API key from `$TOGETHER_API_KEY` (R-01-063).
pub fn together_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(env_key("Together API key", [TOGETHER_API_KEY_ENV]))
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::auth::types::AuthContext;
    use crate::context::Context;
    use crate::provider::Provider;
    use crate::stream::{StreamOptions, collect_message};
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
        assert!(models.iter().all(|m| m.base_url == TOGETHER_BASE_URL));
        assert!(p.get_model("openai/gpt-oss-120b").is_some());
    }

    #[test]
    fn full_catalog_ported_verbatim_from_pi() {
        let models = together_models();
        // Every model id present in Pi's `together.models.ts` (20 entries).
        assert_eq!(models.len(), 20);
        let find = |id: &str| {
            models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("missing {id}"))
        };

        // gpt-oss: openai thinking format + reasoning_effort supported.
        let gpt = find("openai/gpt-oss-120b");
        let gc = gpt.compat.as_ref().unwrap();
        assert_eq!(gc.thinking_format, Some(ThinkingFormat::Openai));
        assert_eq!(gc.supports_reasoning_effort, Some(true));
        assert_eq!(gc.max_tokens_field, Some(MaxTokensField::MaxTokens));
        assert_eq!(gpt.cost.input, 0.15);

        // DeepSeek: together format but reasoning_effort supported, high->"high" map.
        let ds = find("deepseek-ai/DeepSeek-V4-Pro");
        let dc = ds.compat.as_ref().unwrap();
        assert_eq!(dc.thinking_format, Some(ThinkingFormat::Together));
        assert_eq!(dc.supports_reasoning_effort, Some(true));
        assert_eq!(
            ds.thinking_level_map.as_ref().unwrap().get("high"),
            Some(&Some("high".to_string()))
        );
        assert_eq!(ds.context_window, 512_000);
        assert_eq!(ds.max_tokens, 384_000);

        // MiniMax-M2.7 omits thinkingFormat (=> detected "together").
        let m27 = find("MiniMaxAI/MiniMax-M2.7");
        assert_eq!(m27.compat.as_ref().unwrap().thinking_format, None);

        // MiniMax-M3 is vision-capable with the larger context window.
        let m3 = find("MiniMaxAI/MiniMax-M3");
        assert!(m3.supports_image_input());
        assert_eq!(m3.context_window, 524_288);
        assert_eq!(m3.max_tokens, 250_000);

        // Standard together models: store/developer/strict/long-cache all off; max_tokens field.
        for id in [
            "zai-org/GLM-5.1",
            "moonshotai/Kimi-K2.6",
            "Qwen/Qwen3.6-Plus",
        ] {
            let c = find(id).compat.as_ref().unwrap();
            assert_eq!(c.supports_store, Some(false));
            assert_eq!(c.supports_developer_role, Some(false));
            assert_eq!(c.supports_strict_mode, Some(false));
            assert_eq!(c.supports_long_cache_retention, Some(false));
            assert_eq!(c.max_tokens_field, Some(MaxTokensField::MaxTokens));
            assert_eq!(c.thinking_format, Some(ThinkingFormat::Together));
        }
    }

    #[test]
    fn catalog_models_encode_reasoning_per_pi() {
        use crate::api::openai_completions::build_body;
        use crate::context::Context;
        use cyrup_core::ModelThinkingLevel;
        let models = together_models();
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            max_tokens: Some(50),
            ..Default::default()
        };

        // A `together` thinkingFormat model => reasoning: { enabled }, never reasoning_effort.
        let glm = models
            .iter()
            .find(|m| m.id.as_str() == "zai-org/GLM-5")
            .unwrap();
        let body = build_body(glm, &Context::default(), &opts);
        assert_eq!(body["reasoning"], serde_json::json!({ "enabled": true }));
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["max_tokens"], 50);
        assert!(body.get("max_completion_tokens").is_none());

        // gpt-oss (openai format + reasoning_effort) => reasoning_effort, no reasoning object.
        let gpt = models
            .iter()
            .find(|m| m.id.as_str() == "openai/gpt-oss-120b")
            .unwrap();
        let body = build_body(gpt, &Context::default(), &opts);
        assert_eq!(body["reasoning_effort"], "high");
        assert!(body.get("reasoning").is_none());

        // DeepSeek (together format + reasoning_effort supported) => BOTH reasoning.enabled and
        // reasoning_effort mapped via thinkingLevelMap (high -> "high").
        let ds = models
            .iter()
            .find(|m| m.id.as_str() == "deepseek-ai/DeepSeek-V4-Pro")
            .unwrap();
        let body = build_body(ds, &Context::default(), &opts);
        assert_eq!(body["reasoning"], serde_json::json!({ "enabled": true }));
        assert_eq!(body["reasoning_effort"], "high");
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
        model.base_url = "http://127.0.0.1:1/v1".to_string();

        let msg = collect_message(provider.stream(
            &model,
            &Context::default(),
            &StreamOptions::default(),
        ))
        .await;
        // Auth resolved (no "not configured"); failure is a transport error terminal.
        assert_eq!(msg.stop_reason, StopReason::Error);
        let err = msg.error_message.unwrap();
        assert!(
            !err.contains("not configured"),
            "auth should have resolved, got: {err}"
        );
        assert!(
            err.contains("transport"),
            "expected transport error, got: {err}"
        );
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
        // successful round-trips. Only a transport/API failure yields `Error`. Sending reasoning
        // `High` exercises the live reasoning encoding (openai format => `reasoning_effort`) and
        // the full compat-driven body (`max_tokens`, no `store`) against the real API.
        let opts = StreamOptions {
            max_tokens: Some(256),
            reasoning: cyrup_core::ModelThinkingLevel::High,
            ..Default::default()
        };
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
        assert!(
            has_content,
            "expected non-empty assistant content, got: {:?}",
            msg.content
        );
    }
}
