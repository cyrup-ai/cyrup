//! Provider → env-var API-key map (1:1 port of Pi `packages/ai/src/env-api-keys.ts`).
//!
//! Maps a provider id to the environment variable(s) that can supply its API key, plus the two
//! ambient-credential providers (Vertex ADC, Amazon Bedrock) that authenticate without a literal
//! key. Mirrors `getApiKeyEnvVars`, `findEnvKeys`, and `getEnvApiKey`, and the `getProviderEnvValue`
//! overlay precedence from `utils/provider-env.ts`.

use crate::auth::types::{AuthContext, ProviderEnv};

/// Resolve a provider env value: a non-empty scoped overlay wins, else the ambient context env.
/// 1:1 port of `getProviderEnvValue` (`utils/provider-env.ts:45`): `env?.[name] || process.env[name]`
/// (JS `||` skips empty strings, so an empty overlay falls through to the ambient value).
pub async fn get_provider_env_value(
    name: &str,
    ctx: &dyn AuthContext,
    env: Option<&ProviderEnv>,
) -> Option<String> {
    if let Some(overlay) = env
        && let Some(v) = overlay.get(name)
        && !v.is_empty()
    {
        return Some(v.clone());
    }
    match ctx.env(name).await {
        Some(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// The API-key env var(s) for a provider, in precedence order (first present wins).
/// 1:1 port of `getApiKeyEnvVars` (`env-api-keys.ts:64`). Returns `None` for providers that have no
/// literal-key env var (OAuth-only providers other than the special-cased ones, ambient-only
/// providers, keyless-local providers).
pub fn api_key_env_vars(provider: &str) -> Option<&'static [&'static str]> {
    match provider {
        // github-copilot uses an OAuth token carried in COPILOT_GITHUB_TOKEN (env-api-keys.ts:65).
        "github-copilot" => Some(&["COPILOT_GITHUB_TOKEN"]),
        // ANTHROPIC_OAUTH_TOKEN takes precedence over ANTHROPIC_API_KEY (env-api-keys.ts:70).
        "anthropic" => Some(&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]),
        "ant-ling" => Some(&["ANT_LING_API_KEY"]),
        "openai" => Some(&["OPENAI_API_KEY"]),
        "azure-openai-responses" => Some(&["AZURE_OPENAI_API_KEY"]),
        "nvidia" => Some(&["NVIDIA_API_KEY"]),
        "deepseek" => Some(&["DEEPSEEK_API_KEY"]),
        "google" => Some(&["GEMINI_API_KEY"]),
        "google-vertex" => Some(&["GOOGLE_CLOUD_API_KEY"]),
        "groq" => Some(&["GROQ_API_KEY"]),
        "cerebras" => Some(&["CEREBRAS_API_KEY"]),
        "xai" => Some(&["XAI_API_KEY"]),
        "openrouter" => Some(&["OPENROUTER_API_KEY"]),
        "vercel-ai-gateway" => Some(&["AI_GATEWAY_API_KEY"]),
        "zai" => Some(&["ZAI_API_KEY"]),
        "zai-coding-cn" => Some(&["ZAI_CODING_CN_API_KEY"]),
        "mistral" => Some(&["MISTRAL_API_KEY"]),
        "minimax" => Some(&["MINIMAX_API_KEY"]),
        "minimax-cn" => Some(&["MINIMAX_CN_API_KEY"]),
        "moonshotai" => Some(&["MOONSHOT_API_KEY"]),
        "moonshotai-cn" => Some(&["MOONSHOT_API_KEY"]),
        "huggingface" => Some(&["HF_TOKEN"]),
        "fireworks" => Some(&["FIREWORKS_API_KEY"]),
        "together" => Some(&["TOGETHER_API_KEY"]),
        "opencode" => Some(&["OPENCODE_API_KEY"]),
        "opencode-go" => Some(&["OPENCODE_API_KEY"]),
        "kimi-coding" => Some(&["KIMI_API_KEY"]),
        "cloudflare-workers-ai" => Some(&["CLOUDFLARE_API_KEY"]),
        "cloudflare-ai-gateway" => Some(&["CLOUDFLARE_API_KEY"]),
        "xiaomi" => Some(&["XIAOMI_API_KEY"]),
        "xiaomi-token-plan-cn" => Some(&["XIAOMI_TOKEN_PLAN_CN_API_KEY"]),
        "xiaomi-token-plan-ams" => Some(&["XIAOMI_TOKEN_PLAN_AMS_API_KEY"]),
        "xiaomi-token-plan-sgp" => Some(&["XIAOMI_TOKEN_PLAN_SGP_API_KEY"]),
        _ => None,
    }
}

/// The configured env var(s) that can provide an API key for a provider.
/// 1:1 port of `findEnvKeys` (`env-api-keys.ts:121`). Reports only literal API-key vars (it
/// intentionally excludes ambient sources: AWS profiles/IAM, Google ADC). Returns `None` when none
/// of the provider's vars are present.
pub async fn find_env_keys(
    provider: &str,
    ctx: &dyn AuthContext,
    env: Option<&ProviderEnv>,
) -> Option<Vec<String>> {
    let vars = api_key_env_vars(provider)?;
    let mut found = Vec::new();
    for var in vars {
        if get_provider_env_value(var, ctx, env).await.is_some() {
            found.push((*var).to_string());
        }
    }
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// Sentinel returned for ambient-credential providers (Vertex ADC / Bedrock) that are configured
/// without a literal key. 1:1 with Pi's `"<authenticated>"`.
pub const AUTHENTICATED_SENTINEL: &str = "<authenticated>";

/// Get the API key for a provider from known env vars (e.g. `OPENAI_API_KEY`).
/// 1:1 port of `getEnvApiKey` (`env-api-keys.ts:136`). Will not return keys for OAuth-only
/// providers; returns the `"<authenticated>"` sentinel for Vertex ADC / Bedrock ambient creds.
pub async fn get_env_api_key(
    provider: &str,
    ctx: &dyn AuthContext,
    env: Option<&ProviderEnv>,
) -> Option<String> {
    if let Some(keys) = find_env_keys(provider, ctx, env).await
        && let Some(first) = keys.first()
    {
        return get_provider_env_value(first, ctx, env).await;
    }

    // Vertex AI: explicit api key OR Application Default Credentials (env-api-keys.ts:144).
    if provider == "google-vertex" {
        let has_credentials = has_vertex_adc_credentials(ctx, env).await;
        let has_project = get_provider_env_value("GOOGLE_CLOUD_PROJECT", ctx, env).await.is_some()
            || get_provider_env_value("GCLOUD_PROJECT", ctx, env).await.is_some();
        let has_location =
            get_provider_env_value("GOOGLE_CLOUD_LOCATION", ctx, env).await.is_some();
        if has_credentials && has_project && has_location {
            return Some(AUTHENTICATED_SENTINEL.to_string());
        }
    }

    // Amazon Bedrock: multiple ambient credential sources (env-api-keys.ts:156).
    if provider == "amazon-bedrock" {
        let v = |name: &'static str| get_provider_env_value(name, ctx, env);
        if v("AWS_PROFILE").await.is_some()
            || (v("AWS_ACCESS_KEY_ID").await.is_some()
                && v("AWS_SECRET_ACCESS_KEY").await.is_some())
            || v("AWS_BEARER_TOKEN_BEDROCK").await.is_some()
            || v("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI").await.is_some()
            || v("AWS_CONTAINER_CREDENTIALS_FULL_URI").await.is_some()
            || v("AWS_WEB_IDENTITY_TOKEN_FILE").await.is_some()
        {
            return Some(AUTHENTICATED_SENTINEL.to_string());
        }
    }

    None
}

/// Detect Google Application Default Credentials (1:1 with `hasVertexAdcCredentials`,
/// `env-api-keys.ts:31`): an explicit `GOOGLE_APPLICATION_CREDENTIALS` path, else the default
/// `~/.config/gcloud/application_default_credentials.json`.
pub async fn has_vertex_adc_credentials(
    ctx: &dyn AuthContext,
    env: Option<&ProviderEnv>,
) -> bool {
    if let Some(path) = get_provider_env_value("GOOGLE_APPLICATION_CREDENTIALS", ctx, env).await {
        return ctx.file_exists(&path).await;
    }
    // Default ADC location under the home directory.
    if let Some(home) = ctx.env("HOME").await.filter(|h| !h.is_empty()) {
        let default_path =
            format!("{home}/.config/gcloud/application_default_credentials.json");
        return ctx.file_exists(&default_path).await;
    }
    false
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct MapCtx {
        env: BTreeMap<String, String>,
        files: Vec<String>,
    }
    #[async_trait::async_trait]
    impl AuthContext for MapCtx {
        async fn env(&self, name: &str) -> Option<String> {
            self.env.get(name).cloned()
        }
        async fn file_exists(&self, path: &str) -> bool {
            self.files.iter().any(|f| f == path)
        }
    }
    fn ctx(pairs: &[(&str, &str)]) -> MapCtx {
        MapCtx {
            env: pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect(),
            files: Vec::new(),
        }
    }

    #[test]
    fn map_covers_every_fleet_provider() {
        for p in [
            "openai", "anthropic", "google", "groq", "cerebras", "xai", "openrouter", "deepseek",
            "nvidia", "moonshotai", "moonshotai-cn", "zai", "zai-coding-cn", "ant-ling",
            "huggingface", "together", "fireworks", "mistral", "minimax", "github-copilot",
            "xiaomi", "xiaomi-token-plan-cn", "xiaomi-token-plan-ams", "xiaomi-token-plan-sgp",
        ] {
            assert!(api_key_env_vars(p).is_some(), "missing env-key mapping for {p}");
        }
        assert!(api_key_env_vars("does-not-exist").is_none());
    }

    #[tokio::test]
    async fn anthropic_oauth_token_takes_precedence() {
        // Both present: OAUTH token wins (it is listed first).
        let c = ctx(&[("ANTHROPIC_OAUTH_TOKEN", "oauth-tok"), ("ANTHROPIC_API_KEY", "sk-ant")]);
        assert_eq!(get_env_api_key("anthropic", &c, None).await.as_deref(), Some("oauth-tok"));
        // Only the api key present: it is used.
        let c = ctx(&[("ANTHROPIC_API_KEY", "sk-ant")]);
        assert_eq!(get_env_api_key("anthropic", &c, None).await.as_deref(), Some("sk-ant"));
    }

    #[tokio::test]
    async fn moonshot_cn_shares_moonshot_key() {
        let c = ctx(&[("MOONSHOT_API_KEY", "ms-key")]);
        assert_eq!(get_env_api_key("moonshotai", &c, None).await.as_deref(), Some("ms-key"));
        assert_eq!(get_env_api_key("moonshotai-cn", &c, None).await.as_deref(), Some("ms-key"));
    }

    #[tokio::test]
    async fn overlay_beats_process_env() {
        let c = ctx(&[("GROQ_API_KEY", "from-process")]);
        let mut overlay = ProviderEnv::new();
        overlay.insert("GROQ_API_KEY".to_string(), "from-overlay".to_string());
        assert_eq!(
            get_env_api_key("groq", &c, Some(&overlay)).await.as_deref(),
            Some("from-overlay")
        );
        // Empty overlay value falls through to process env (JS `||` semantics).
        overlay.insert("GROQ_API_KEY".to_string(), String::new());
        assert_eq!(
            get_env_api_key("groq", &c, Some(&overlay)).await.as_deref(),
            Some("from-process")
        );
    }

    #[tokio::test]
    async fn unconfigured_returns_none() {
        let c = ctx(&[]);
        assert!(get_env_api_key("groq", &c, None).await.is_none());
        assert!(find_env_keys("groq", &c, None).await.is_none());
    }

    #[tokio::test]
    async fn vertex_requires_credentials_project_and_location() {
        // Explicit api key path short-circuits to the literal key.
        let c = ctx(&[("GOOGLE_CLOUD_API_KEY", "vk")]);
        assert_eq!(get_env_api_key("google-vertex", &c, None).await.as_deref(), Some("vk"));

        // ADC sentinel only when credentials + project + location are all present.
        let mut m = MapCtx {
            env: BTreeMap::from([
                (
                    "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
                    "/creds/adc.json".to_string(),
                ),
                ("GOOGLE_CLOUD_PROJECT".to_string(), "proj".to_string()),
                ("GOOGLE_CLOUD_LOCATION".to_string(), "us-central1".to_string()),
            ]),
            files: vec!["/creds/adc.json".to_string()],
        };
        assert_eq!(
            get_env_api_key("google-vertex", &m, None).await.as_deref(),
            Some(AUTHENTICATED_SENTINEL)
        );
        // Missing location → not configured.
        m.env.remove("GOOGLE_CLOUD_LOCATION");
        assert!(get_env_api_key("google-vertex", &m, None).await.is_none());
    }

    #[tokio::test]
    async fn bedrock_ambient_credentials_detected() {
        let c = ctx(&[("AWS_PROFILE", "default")]);
        assert_eq!(
            get_env_api_key("amazon-bedrock", &c, None).await.as_deref(),
            Some(AUTHENTICATED_SENTINEL)
        );
        // IAM pair requires BOTH keys.
        let c = ctx(&[("AWS_ACCESS_KEY_ID", "id")]);
        assert!(get_env_api_key("amazon-bedrock", &c, None).await.is_none());
        let c = ctx(&[("AWS_ACCESS_KEY_ID", "id"), ("AWS_SECRET_ACCESS_KEY", "sec")]);
        assert_eq!(
            get_env_api_key("amazon-bedrock", &c, None).await.as_deref(),
            Some(AUTHENTICATED_SENTINEL)
        );
    }
}
