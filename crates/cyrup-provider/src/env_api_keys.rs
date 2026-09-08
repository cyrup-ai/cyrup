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
        // PROV-021. `if (provider === "anthropic") return [ANTHROPIC_AUTH_TOKEN_ENV,
        // ANTHROPIC_OAUTH_TOKEN_ENV, ANTHROPIC_API_KEY_ENV]` (env-api-keys.ts:73-76 @v0.83.0), with
        // the inline carve-out comment that `ANTHROPIC_AUTH_TOKEN` "participates in env
        // discovery/status, but getEnvApiKey() skips it because requests must pass it as
        // Authorization: Bearer". The skip is implemented in [`get_env_api_key`].
        "anthropic" => Some(&[
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_OAUTH_TOKEN",
            "ANTHROPIC_API_KEY",
        ]),
        "ant-ling" => Some(&["ANT_LING_API_KEY"]),
        // PROV-014. `"qwen-token-plan": "QWEN_TOKEN_PLAN_API_KEY"` /
        // `"qwen-token-plan-cn": "QWEN_TOKEN_PLAN_CN_API_KEY"` (env-api-keys.ts:80-81 @v0.83.0) —
        // the two entries pi's `envMap` places immediately after `ant-ling`, kept in upstream's
        // order here so the two maps stay diffable.
        "qwen-token-plan" => Some(&["QWEN_TOKEN_PLAN_API_KEY"]),
        "qwen-token-plan-cn" => Some(&["QWEN_TOKEN_PLAN_CN_API_KEY"]),
        // VERSION LAG (v0.83.0 → v0.84.4): `"qwen-token-plan-individual": "QWEN_TOKEN_PLAN_API_KEY"`
        // (env-api-keys.ts:83 @v0.84.4) — deliberately the SAME variable as `qwen-token-plan`
        // (`qwen-token-plan-models.test.ts:121-125`: "reuses the international Token Plan
        // environment variable"). PROV-014.
        "qwen-token-plan-individual" => Some(&["QWEN_TOKEN_PLAN_API_KEY"]),
        "openai" => Some(&["OPENAI_API_KEY"]),
        "azure-openai-responses" => Some(&["AZURE_OPENAI_API_KEY"]),
        "nvidia" => Some(&["NVIDIA_API_KEY"]),
        "deepseek" => Some(&["DEEPSEEK_API_KEY"]),
        "google" => Some(&["GEMINI_API_KEY"]),
        "google-vertex" => Some(&["GOOGLE_CLOUD_API_KEY"]),
        "groq" => Some(&["GROQ_API_KEY"]),
        "cerebras" => Some(&["CEREBRAS_API_KEY"]),
        "xai" => Some(&["XAI_API_KEY"]),
        // PROV-014. `radius: "RADIUS_API_KEY"` (env-api-keys.ts:91 @v0.83.0), between `xai` and
        // `openrouter` upstream.
        "radius" => Some(&["RADIUS_API_KEY"]),
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
        // DRIFT-009. `baseten: "BASETEN_API_KEY"` (env-api-keys.ts:106 @v0.84.4), between
        // `together` and `opencode` upstream. A v0.84.x addition — the key is absent from
        // `env-api-keys.ts` at v0.83.0, as is the provider.
        "baseten" => Some(&["BASETEN_API_KEY"]),
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

/// Every environment variable this module can turn into a usable provider credential — the
/// UNION of every [`api_key_env_vars`] arm plus the ambient-credential triggers consulted by
/// [`get_env_api_key`] / [`has_vertex_adc_credentials`] (Vertex ADC, Amazon Bedrock), plus the
/// AWS companions the Bedrock SDK itself reads once a request is signed (`AWS_SESSION_TOKEN`,
/// `AWS_REGION`, `AWS_DEFAULT_REGION`).
///
/// # This is a TEST-HERMETICITY contract, kept here on purpose
///
/// The integration suite (`crates/cyrup-it/tests/support/env.rs`) derives its credential scrub
/// from THIS slice, so "which env vars can spend real tokens" has exactly one owner: the crate
/// that reads them. A hand-copied list over there is how `TOGETHER_API_KEY` once reached a child
/// process and made a real network call — the denylist knew 4 names, this map knew ~40.
///
/// The `credential_env_inventory_covers_every_name_this_file_reads` test locks the slice to this
/// file's own source: any new `SOMETHING_API_KEY` literal added to [`api_key_env_vars`] (or
/// anywhere else in this module) fails that test until it is added here too. Entries may be a
/// SUPERSET of what the file names (the AWS companions are), never a subset.
pub const CREDENTIAL_ENV_VARS: &[&str] = &[
    // -- literal API keys / tokens, one per `api_key_env_vars` arm --------------------------
    "COPILOT_GITHUB_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_OAUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANT_LING_API_KEY",
    "QWEN_TOKEN_PLAN_API_KEY",
    "QWEN_TOKEN_PLAN_CN_API_KEY",
    "OPENAI_API_KEY",
    "AZURE_OPENAI_API_KEY",
    "NVIDIA_API_KEY",
    "DEEPSEEK_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_CLOUD_API_KEY",
    "GROQ_API_KEY",
    "CEREBRAS_API_KEY",
    "XAI_API_KEY",
    "RADIUS_API_KEY",
    "OPENROUTER_API_KEY",
    "AI_GATEWAY_API_KEY",
    "ZAI_API_KEY",
    "ZAI_CODING_CN_API_KEY",
    "MISTRAL_API_KEY",
    "MINIMAX_API_KEY",
    "MINIMAX_CN_API_KEY",
    "MOONSHOT_API_KEY",
    "HF_TOKEN",
    "FIREWORKS_API_KEY",
    "TOGETHER_API_KEY",
    "BASETEN_API_KEY",
    "OPENCODE_API_KEY",
    "KIMI_API_KEY",
    "CLOUDFLARE_API_KEY",
    "XIAOMI_API_KEY",
    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
    // -- Google Vertex ambient credentials (ADC), `get_env_api_key` + `has_vertex_adc_credentials`
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_CLOUD_PROJECT",
    "GCLOUD_PROJECT",
    "GOOGLE_CLOUD_LOCATION",
    // -- Amazon Bedrock ambient credentials (`get_env_api_key`'s `amazon-bedrock` block) -----
    "AWS_PROFILE",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    // -- AWS companions not named in this file but read by the signing SDK -------------------
    "AWS_SESSION_TOKEN",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
];

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
    if found.is_empty() { None } else { Some(found) }
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
        && !keys.is_empty()
    {
        // `const apiKeyEnv = provider === "anthropic" ? envKeys.find(key => key !==
        // ANTHROPIC_AUTH_TOKEN_ENV) : envKeys[0]` (env-api-keys.ts:147 @v0.83.0) — PROV-021.
        // `ANTHROPIC_AUTH_TOKEN` is discoverable (so auth STATUS reports it) but must never be
        // turned into a literal api key, because it has to travel as `Authorization: Bearer`.
        let api_key_env = if provider == "anthropic" {
            keys.iter().find(|k| k.as_str() != "ANTHROPIC_AUTH_TOKEN")
        } else {
            keys.first()
        };
        if let Some(name) = api_key_env {
            return get_provider_env_value(name, ctx, env).await;
        }
    }

    // Vertex AI: explicit api key OR Application Default Credentials (env-api-keys.ts:144).
    if provider == "google-vertex" {
        let has_credentials = has_vertex_adc_credentials(ctx, env).await;
        let has_project = get_provider_env_value("GOOGLE_CLOUD_PROJECT", ctx, env)
            .await
            .is_some()
            || get_provider_env_value("GCLOUD_PROJECT", ctx, env)
                .await
                .is_some();
        let has_location = get_provider_env_value("GOOGLE_CLOUD_LOCATION", ctx, env)
            .await
            .is_some();
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
pub async fn has_vertex_adc_credentials(ctx: &dyn AuthContext, env: Option<&ProviderEnv>) -> bool {
    if let Some(path) = get_provider_env_value("GOOGLE_APPLICATION_CREDENTIALS", ctx, env).await {
        return ctx.file_exists(&path).await;
    }
    // Default ADC location under the home directory.
    if let Some(home) = ctx.env("HOME").await.filter(|h| !h.is_empty()) {
        let default_path = format!("{home}/.config/gcloud/application_default_credentials.json");
        return ctx.file_exists(&default_path).await;
    }
    false
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
            env: pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            files: Vec::new(),
        }
    }

    #[test]
    fn map_covers_every_fleet_provider() {
        for p in [
            "openai",
            "anthropic",
            "google",
            "groq",
            "cerebras",
            "xai",
            "openrouter",
            "deepseek",
            "nvidia",
            "moonshotai",
            "moonshotai-cn",
            "zai",
            "zai-coding-cn",
            "ant-ling",
            "huggingface",
            "together",
            "fireworks",
            "mistral",
            "minimax",
            "github-copilot",
            "xiaomi",
            "xiaomi-token-plan-cn",
            "xiaomi-token-plan-ams",
            "xiaomi-token-plan-sgp",
            "qwen-token-plan",
            "qwen-token-plan-cn",
            "qwen-token-plan-individual",
            "radius",
            "baseten",
        ] {
            assert!(
                api_key_env_vars(p).is_some(),
                "missing env-key mapping for {p}"
            );
        }
        assert!(api_key_env_vars("does-not-exist").is_none());
    }

    /// PROV-014 — the three Qwen Token Plan rows and radius, key for key against
    /// `env-api-keys.ts:81-83`,`:93` @v0.84.4. The Individual plan shares the international
    /// variable (`qwen-token-plan-models.test.ts:121-125` @v0.84.4).
    #[test]
    fn qwen_token_plan_and_radius_rows_match_upstream() {
        assert_eq!(
            api_key_env_vars("qwen-token-plan"),
            Some(&["QWEN_TOKEN_PLAN_API_KEY"][..])
        );
        assert_eq!(
            api_key_env_vars("qwen-token-plan-cn"),
            Some(&["QWEN_TOKEN_PLAN_CN_API_KEY"][..])
        );
        assert_eq!(
            api_key_env_vars("qwen-token-plan-individual"),
            Some(&["QWEN_TOKEN_PLAN_API_KEY"][..])
        );
        assert_eq!(api_key_env_vars("radius"), Some(&["RADIUS_API_KEY"][..]));
    }

    /// DRIFT-009 — `baseten: "BASETEN_API_KEY"` (`env-api-keys.ts:106` @v0.84.4). Its own
    /// variable, shared with nothing, and absent at v0.83.0 along with the provider.
    #[test]
    fn baseten_row_matches_upstream() {
        assert_eq!(
            api_key_env_vars("baseten"),
            Some(&["BASETEN_API_KEY"][..]),
            "env-api-keys.ts:106 @v0.84.4"
        );
    }

    #[tokio::test]
    async fn anthropic_oauth_token_takes_precedence() {
        // Both present: OAUTH token wins (it is listed first).
        let c = ctx(&[
            ("ANTHROPIC_OAUTH_TOKEN", "oauth-tok"),
            ("ANTHROPIC_API_KEY", "sk-ant"),
        ]);
        assert_eq!(
            get_env_api_key("anthropic", &c, None).await.as_deref(),
            Some("oauth-tok")
        );
        // Only the api key present: it is used.
        let c = ctx(&[("ANTHROPIC_API_KEY", "sk-ant")]);
        assert_eq!(
            get_env_api_key("anthropic", &c, None).await.as_deref(),
            Some("sk-ant")
        );
    }

    #[tokio::test]
    async fn moonshot_cn_shares_moonshot_key() {
        let c = ctx(&[("MOONSHOT_API_KEY", "ms-key")]);
        assert_eq!(
            get_env_api_key("moonshotai", &c, None).await.as_deref(),
            Some("ms-key")
        );
        assert_eq!(
            get_env_api_key("moonshotai-cn", &c, None).await.as_deref(),
            Some("ms-key")
        );
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
        assert_eq!(
            get_env_api_key("google-vertex", &c, None).await.as_deref(),
            Some("vk")
        );

        // ADC sentinel only when credentials + project + location are all present.
        let mut m = MapCtx {
            env: BTreeMap::from([
                (
                    "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
                    "/creds/adc.json".to_string(),
                ),
                ("GOOGLE_CLOUD_PROJECT".to_string(), "proj".to_string()),
                (
                    "GOOGLE_CLOUD_LOCATION".to_string(),
                    "us-central1".to_string(),
                ),
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

    /// The [`CREDENTIAL_ENV_VARS`] sync guard: every UPPER_SNAKE string literal in this file —
    /// which is where EVERY provider-credential env var this crate reads is spelled — must be in
    /// the inventory. A new `api_key_env_vars` arm whose variable is missing from
    /// [`CREDENTIAL_ENV_VARS`] fails here, which is what keeps the integration suite's scrub
    /// (derived from that slice) complete without a human remembering two lists.
    ///
    /// Scanning SOURCE rather than calling the function is deliberate: a `match` cannot be
    /// enumerated at runtime, and a hand-kept provider-id list is exactly the drift this guard
    /// exists to rule out. Comments are scanned too — a false positive there costs one inventory
    /// entry; a false negative on the real map costs real tokens.
    #[test]
    fn credential_env_inventory_covers_every_name_this_file_reads() {
        let source = include_str!("env_api_keys.rs");
        // Env-var names that appear in this file but are NOT provider credentials.
        let exceptions = ["HOME", "CREDENTIAL_ENV_VARS", "UPPER_SNAKE", "SOMETHING_API_KEY"];

        let mut missing = Vec::new();
        for raw in source.split('"').skip(1).step_by(2) {
            let looks_like_env_var = raw.len() >= 3
                && raw.contains('_')
                && raw
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                && raw.starts_with(|c: char| c.is_ascii_uppercase());
            if looks_like_env_var
                && !exceptions.contains(&raw)
                && !CREDENTIAL_ENV_VARS.contains(&raw)
                && !missing.contains(&raw)
            {
                missing.push(raw);
            }
        }
        assert!(
            missing.is_empty(),
            "env var name(s) read by this module but absent from CREDENTIAL_ENV_VARS: \
             {missing:?}. Add them — the integration suite derives its credential scrub from that \
             slice, and a name missing there can reach a spawned child and spend real tokens."
        );
    }

    /// The inventory covers what [`api_key_env_vars`] returns for every known provider — the
    /// direct (function-level) half of the source-scan above, using the same provider list
    /// [`map_covers_every_fleet_provider`] maintains plus the arms it omits.
    #[test]
    fn credential_env_inventory_covers_every_mapped_provider() {
        for p in [
            "github-copilot",
            "anthropic",
            "ant-ling",
            "qwen-token-plan",
            "qwen-token-plan-cn",
            "qwen-token-plan-individual",
            "openai",
            "azure-openai-responses",
            "nvidia",
            "deepseek",
            "google",
            "google-vertex",
            "groq",
            "cerebras",
            "xai",
            "radius",
            "openrouter",
            "vercel-ai-gateway",
            "zai",
            "zai-coding-cn",
            "mistral",
            "minimax",
            "minimax-cn",
            "moonshotai",
            "moonshotai-cn",
            "huggingface",
            "fireworks",
            "together",
            "baseten",
            "opencode",
            "opencode-go",
            "kimi-coding",
            "cloudflare-workers-ai",
            "cloudflare-ai-gateway",
            "xiaomi",
            "xiaomi-token-plan-cn",
            "xiaomi-token-plan-ams",
            "xiaomi-token-plan-sgp",
        ] {
            let vars = api_key_env_vars(p)
                .unwrap_or_else(|| panic!("provider {p} lost its env-key mapping"));
            for v in vars {
                assert!(
                    CREDENTIAL_ENV_VARS.contains(v),
                    "api_key_env_vars({p:?}) names {v} but CREDENTIAL_ENV_VARS does not carry it"
                );
            }
        }
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
        let c = ctx(&[
            ("AWS_ACCESS_KEY_ID", "id"),
            ("AWS_SECRET_ACCESS_KEY", "sec"),
        ]);
        assert_eq!(
            get_env_api_key("amazon-bedrock", &c, None).await.as_deref(),
            Some(AUTHENTICATED_SENTINEL)
        );
    }
}
