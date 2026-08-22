//! Provider → environment-variable mapping (Pi `ai/src/env-api-keys.ts`).
//!
//! Ports `getApiKeyEnvVars` / `findEnvKeys` / `getEnvApiKey` (env-api-keys.ts:64-177): the canonical
//! per-provider API-key env-var names plus the Vertex-ADC / Bedrock ambient-credential probes. This
//! feeds the `env` tier of [`crate::auth::resolve_auth`] and the `getAuthStatus` label.

use std::collections::HashMap;

/// The ambient tier consulted by [`provider_env_value`] when the scoped overlay does not supply a
/// value: the real process environment in production, a fixed map under test.
///
/// This seam exists because without it the overlay is only half a seam. `provider_env_value` is
/// `env?.[name] || process.env[name]` (Pi `getProviderEnvValue`), so an overlay can only ever ADD a
/// value — it cannot express "this var is unset", and every assertion that a var is absent silently
/// became an assertion about the machine running the suite. That is not hypothetical: it is why
/// `bedrock_ambient_credentials_sentinel` failed on any host with ambient `AWS_SECRET_ACCESS_KEY`,
/// and why `find_and_get_from_scoped_env_map` had to read the ambient value and assert agreement
/// with it instead of asserting the contract. This crate is `#![forbid(unsafe_code)]` and
/// `std::env::remove_var` is unsafe in Rust 2024, so the ambient value cannot be scrubbed
/// in-process — it has to be injectable instead.
///
/// `cyrup-provider`'s twin of this module already gets exactly this seam from
/// [`AuthContext::env`](cyrup_provider::auth::types::AuthContext), which is why its
/// `bedrock_ambient_credentials_detected` is hermetic where this module's was not. Same shape,
/// same precedence — overlay first, then ambient, empty treated as unset at both tiers.
#[derive(Clone, Copy)]
enum Ambient<'a> {
    /// Production: read the real process environment.
    Process,
    /// Test: read a fixed map, so "unset" is a property of the fixture, not of the host.
    /// Only ever constructed under `cfg(test)` — production always resolves through
    /// [`Self::Process`], so this carries no runtime cost and changes no shipped behavior.
    #[cfg_attr(not(test), allow(dead_code))]
    Fixed(&'a HashMap<String, String>),
}

impl Ambient<'_> {
    fn get(self, name: &str) -> Option<String> {
        match self {
            Self::Process => std::env::var(name).ok().filter(|v| !v.is_empty()),
            Self::Fixed(map) => map.get(name).filter(|v| !v.is_empty()).cloned(),
        }
    }
}

/// Lookup a provider-scoped value, falling back to the ambient environment, treating empty as unset
/// (Pi `getProviderEnvValue`). `env` is an optional override map (used in tests / scoped configs).
fn provider_env_value(
    name: &str,
    env: Option<&HashMap<String, String>>,
    ambient: Ambient<'_>,
) -> Option<String> {
    if let Some(map) = env
        && let Some(v) = map.get(name)
        && !v.is_empty()
    {
        return Some(v.clone());
    }
    ambient.get(name)
}

/// Canonical API-key env-var name(s) for a provider (Pi `getApiKeyEnvVars`, :64-110). `None` for
/// providers without a known key var (OAuth-only / ambient-credential providers).
pub fn api_key_env_vars(provider: &str) -> Option<&'static [&'static str]> {
    // github-copilot and anthropic are special-cased first in Pi.
    if provider == "github-copilot" {
        return Some(&["COPILOT_GITHUB_TOKEN"]);
    }
    // ANTHROPIC_OAUTH_TOKEN takes precedence over ANTHROPIC_API_KEY (:70-72).
    if provider == "anthropic" {
        return Some(&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"]);
    }
    let v: &'static [&'static str] = match provider {
        "ant-ling" => &["ANT_LING_API_KEY"],
        // env-api-keys.ts:81-82 @v0.83.0. Both were MISSING: `default_model_per_provider` and
        // `KNOWN_PROVIDERS` already carry these two ids (CFG-019, `model.rs`), so cyrup knew the
        // providers and then reported a user who had exported `QWEN_TOKEN_PLAN_API_KEY` as having
        // no credential at all — `find_env_keys` returned `None`, so `provider_auth_status` said
        // `configured: false`, `provider_is_configured` skipped the provider at launch steps 1/4,
        // `/login` listed it as unconfigured, and `get_env_api_key` handed the request builder
        // nothing.
        "qwen-token-plan" => &["QWEN_TOKEN_PLAN_API_KEY"],
        "qwen-token-plan-cn" => &["QWEN_TOKEN_PLAN_CN_API_KEY"],
        // env-api-keys.ts:83 @v0.84.1 — deliberately the SAME variable as `qwen-token-plan`, not a
        // `_INDIVIDUAL_` spelling. Upstream drift, landed with the id itself (CFG-041).
        "qwen-token-plan-individual" => &["QWEN_TOKEN_PLAN_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "azure-openai-responses" => &["AZURE_OPENAI_API_KEY"],
        "nvidia" => &["NVIDIA_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "google" => &["GEMINI_API_KEY"],
        "google-vertex" => &["GOOGLE_CLOUD_API_KEY"],
        "groq" => &["GROQ_API_KEY"],
        "cerebras" => &["CEREBRAS_API_KEY"],
        "xai" => &["XAI_API_KEY"],
        // env-api-keys.ts:92 @v0.83.0. Also missing, and `radius` is a first-class id in cyrup:
        // `default_model_per_provider` (`model.rs`) maps it to `auto` and `models.json`'s
        // `oauth: Type.Literal("radius")` is the only accepted oauth value there.
        "radius" => &["RADIUS_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "vercel-ai-gateway" => &["AI_GATEWAY_API_KEY"],
        "zai" => &["ZAI_API_KEY"],
        "zai-coding-cn" => &["ZAI_CODING_CN_API_KEY"],
        "mistral" => &["MISTRAL_API_KEY"],
        "minimax" => &["MINIMAX_API_KEY"],
        "minimax-cn" => &["MINIMAX_CN_API_KEY"],
        "moonshotai" => &["MOONSHOT_API_KEY"],
        "moonshotai-cn" => &["MOONSHOT_API_KEY"],
        "huggingface" => &["HF_TOKEN"],
        "fireworks" => &["FIREWORKS_API_KEY"],
        "together" => &["TOGETHER_API_KEY"],
        // env-api-keys.ts:106 @v0.84.1 — upstream drift, landed with the id itself (CFG-041).
        "baseten" => &["BASETEN_API_KEY"],
        "opencode" => &["OPENCODE_API_KEY"],
        "opencode-go" => &["OPENCODE_API_KEY"],
        "kimi-coding" => &["KIMI_API_KEY"],
        "cloudflare-workers-ai" => &["CLOUDFLARE_API_KEY"],
        "cloudflare-ai-gateway" => &["CLOUDFLARE_API_KEY"],
        "xiaomi" => &["XIAOMI_API_KEY"],
        "xiaomi-token-plan-cn" => &["XIAOMI_TOKEN_PLAN_CN_API_KEY"],
        "xiaomi-token-plan-ams" => &["XIAOMI_TOKEN_PLAN_AMS_API_KEY"],
        "xiaomi-token-plan-sgp" => &["XIAOMI_TOKEN_PLAN_SGP_API_KEY"],
        _ => return None,
    };
    Some(v)
}

/// Find the configured env-var name(s) that can supply an API key for `provider` (Pi `findEnvKeys`,
/// :119-127). Only reports actual key vars (excludes ambient AWS/ADC sources). `None` when none set.
pub fn find_env_keys(provider: &str, env: Option<&HashMap<String, String>>) -> Option<Vec<String>> {
    find_env_keys_in(provider, env, Ambient::Process)
}

fn find_env_keys_in(
    provider: &str,
    env: Option<&HashMap<String, String>>,
    ambient: Ambient<'_>,
) -> Option<Vec<String>> {
    let vars = api_key_env_vars(provider)?;
    let found: Vec<String> = vars
        .iter()
        .filter(|v| provider_env_value(v, env, ambient).is_some())
        .map(|v| v.to_string())
        .collect();
    if found.is_empty() { None } else { Some(found) }
}

/// Whether the default Vertex ADC credentials file exists (Pi `hasVertexAdcCredentials`, :31-62).
fn has_vertex_adc_credentials(env: Option<&HashMap<String, String>>, ambient: Ambient<'_>) -> bool {
    if let Some(explicit) = provider_env_value("GOOGLE_APPLICATION_CREDENTIALS", env, ambient) {
        return std::path::Path::new(&explicit).exists();
    }
    let home = directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from));
    match home {
        Some(h) => h
            .join(".config")
            .join("gcloud")
            .join("application_default_credentials.json")
            .exists(),
        None => false,
    }
}

/// Get an API key for `provider` from known env vars (Pi `getEnvApiKey`, :136-177). Returns the
/// sentinel `"<authenticated>"` for Vertex/Bedrock when their ambient credentials are configured.
pub fn get_env_api_key(provider: &str, env: Option<&HashMap<String, String>>) -> Option<String> {
    get_env_api_key_in(provider, env, Ambient::Process)
}

fn get_env_api_key_in(
    provider: &str,
    env: Option<&HashMap<String, String>>,
    ambient: Ambient<'_>,
) -> Option<String> {
    if let Some(keys) = find_env_keys_in(provider, env, ambient)
        && let Some(first) = keys.first()
    {
        return provider_env_value(first, env, ambient);
    }

    if provider == "google-vertex" {
        let has_credentials = has_vertex_adc_credentials(env, ambient);
        let has_project = provider_env_value("GOOGLE_CLOUD_PROJECT", env, ambient).is_some()
            || provider_env_value("GCLOUD_PROJECT", env, ambient).is_some();
        let has_location = provider_env_value("GOOGLE_CLOUD_LOCATION", env, ambient).is_some();
        if has_credentials && has_project && has_location {
            return Some("<authenticated>".to_string());
        }
    }

    if provider == "amazon-bedrock" {
        let has_aws_keys = provider_env_value("AWS_ACCESS_KEY_ID", env, ambient).is_some()
            && provider_env_value("AWS_SECRET_ACCESS_KEY", env, ambient).is_some();
        if provider_env_value("AWS_PROFILE", env, ambient).is_some()
            || has_aws_keys
            || provider_env_value("AWS_BEARER_TOKEN_BEDROCK", env, ambient).is_some()
            || provider_env_value("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", env, ambient).is_some()
            || provider_env_value("AWS_CONTAINER_CREDENTIALS_FULL_URI", env, ambient).is_some()
            || provider_env_value("AWS_WEB_IDENTITY_TOKEN_FILE", env, ambient).is_some()
        {
            return Some("<authenticated>".to_string());
        }
    }

    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn known_provider_env_var_names() {
        assert_eq!(api_key_env_vars("openai"), Some(&["OPENAI_API_KEY"][..]));
        assert_eq!(
            api_key_env_vars("anthropic"),
            Some(&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"][..])
        );
        assert_eq!(
            api_key_env_vars("github-copilot"),
            Some(&["COPILOT_GITHUB_TOKEN"][..])
        );
        assert_eq!(api_key_env_vars("totally-unknown-provider"), None);
    }

    /// The whole `envMap` (`ai/src/env-api-keys.ts:79-114` @v0.84.1), key for key, so the next
    /// upstream addition fails loudly instead of silently reporting a credentialled provider as
    /// unconfigured.
    ///
    /// Red at HEAD for FIVE rows. Three were baseline misses present at **v0.83.0** —
    /// `qwen-token-plan` (`:81`), `qwen-token-plan-cn` (`:82`) and `radius` (`:92`) — and two are
    /// v0.84.1 additions whose sibling half already landed in `model.rs` under CFG-041:
    /// `qwen-token-plan-individual` (`:83`) and `baseten` (`:106`). All five are ids cyrup already
    /// knows in `default_model_per_provider` / `KNOWN_PROVIDERS`, so the effect was a user with the
    /// documented variable exported being told the provider had no credential.
    #[test]
    fn the_env_var_map_matches_pi_key_for_key() {
        // (provider, env vars) exactly as upstream declares them, in upstream's order.
        let expected: &[(&str, &[&str])] = &[
            ("github-copilot", &["COPILOT_GITHUB_TOKEN"]),
            ("ant-ling", &["ANT_LING_API_KEY"]),
            ("qwen-token-plan", &["QWEN_TOKEN_PLAN_API_KEY"]),
            ("qwen-token-plan-cn", &["QWEN_TOKEN_PLAN_CN_API_KEY"]),
            ("qwen-token-plan-individual", &["QWEN_TOKEN_PLAN_API_KEY"]),
            ("openai", &["OPENAI_API_KEY"]),
            ("azure-openai-responses", &["AZURE_OPENAI_API_KEY"]),
            ("nvidia", &["NVIDIA_API_KEY"]),
            ("deepseek", &["DEEPSEEK_API_KEY"]),
            ("google", &["GEMINI_API_KEY"]),
            ("google-vertex", &["GOOGLE_CLOUD_API_KEY"]),
            ("groq", &["GROQ_API_KEY"]),
            ("cerebras", &["CEREBRAS_API_KEY"]),
            ("xai", &["XAI_API_KEY"]),
            ("radius", &["RADIUS_API_KEY"]),
            ("openrouter", &["OPENROUTER_API_KEY"]),
            ("vercel-ai-gateway", &["AI_GATEWAY_API_KEY"]),
            ("zai", &["ZAI_API_KEY"]),
            ("zai-coding-cn", &["ZAI_CODING_CN_API_KEY"]),
            ("mistral", &["MISTRAL_API_KEY"]),
            ("minimax", &["MINIMAX_API_KEY"]),
            ("minimax-cn", &["MINIMAX_CN_API_KEY"]),
            ("moonshotai", &["MOONSHOT_API_KEY"]),
            ("moonshotai-cn", &["MOONSHOT_API_KEY"]),
            ("huggingface", &["HF_TOKEN"]),
            ("fireworks", &["FIREWORKS_API_KEY"]),
            ("together", &["TOGETHER_API_KEY"]),
            ("baseten", &["BASETEN_API_KEY"]),
            ("opencode", &["OPENCODE_API_KEY"]),
            ("opencode-go", &["OPENCODE_API_KEY"]),
            ("kimi-coding", &["KIMI_API_KEY"]),
            ("cloudflare-workers-ai", &["CLOUDFLARE_API_KEY"]),
            ("cloudflare-ai-gateway", &["CLOUDFLARE_API_KEY"]),
            ("xiaomi", &["XIAOMI_API_KEY"]),
            ("xiaomi-token-plan-cn", &["XIAOMI_TOKEN_PLAN_CN_API_KEY"]),
            ("xiaomi-token-plan-ams", &["XIAOMI_TOKEN_PLAN_AMS_API_KEY"]),
            ("xiaomi-token-plan-sgp", &["XIAOMI_TOKEN_PLAN_SGP_API_KEY"]),
        ];
        for (provider, vars) in expected {
            assert_eq!(
                api_key_env_vars(provider),
                Some(*vars),
                "{provider} does not match pi's envMap"
            );
        }

        // `anthropic` is special-cased ahead of the map (`:75-77`). cyrup carries TWO of upstream's
        // three names: `ANTHROPIC_AUTH_TOKEN` is deliberately absent because a request must send it
        // as `Authorization: Bearer` rather than as an api key, and that provider-side half is
        // PROV-021 / DRIFT-030 in `cyrup-provider`. Listing it here without that half would report
        // a provider as configured and then fail every request — strictly worse than reporting it
        // unconfigured. Pinned so the two halves land together.
        assert_eq!(
            api_key_env_vars("anthropic"),
            Some(&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"][..]),
            "adding ANTHROPIC_AUTH_TOKEN here requires the bearer-header half (PROV-021) and \
             `get_env_api_key`'s skip rule (env-api-keys.ts:147 @v0.83.0) in the same change"
        );
    }

    #[test]
    fn find_and_get_from_scoped_env_map() {
        let env = env_of(&[("OPENAI_API_KEY", "sk-openai")]);
        assert_eq!(
            find_env_keys("openai", Some(&env)),
            Some(vec!["OPENAI_API_KEY".to_string()])
        );
        assert_eq!(
            get_env_api_key("openai", Some(&env)).as_deref(),
            Some("sk-openai")
        );
        // An empty scoped map does NOT mean "unset": `provider_env_value` falls back to the
        // ambient environment, faithfully to Pi's `getProviderEnvValue`, which is
        // `env?.[name] || process.env[name] || …` (`ai/src/utils/provider-env.ts:44-52`).
        //
        // This used to read the real `OPENAI_API_KEY` and assert the two agreed, because the
        // ambient tier was hardcoded to the process env and could not be scrubbed (this crate is
        // `#![forbid(unsafe_code)]` and `std::env::remove_var` is unsafe in Rust 2024). That made
        // the assertion tautological whenever the var was set. With [`Ambient`] injectable the
        // fallback is pinned directly, against a fixture rather than against the host.
        let empty = env_of(&[]);
        let ambient_map = env_of(&[("OPENAI_API_KEY", "sk-ambient")]);
        let ambient = Ambient::Fixed(&ambient_map);
        assert_eq!(
            find_env_keys_in("openai", Some(&empty), ambient),
            Some(vec!["OPENAI_API_KEY".to_string()]),
            "an empty scoped map must defer to the ambient env, not report unset"
        );
        assert_eq!(
            get_env_api_key_in("openai", Some(&empty), ambient).as_deref(),
            Some("sk-ambient")
        );
    }

    #[test]
    fn anthropic_oauth_token_precedence() {
        let env = env_of(&[
            ("ANTHROPIC_API_KEY", "sk-api"),
            ("ANTHROPIC_OAUTH_TOKEN", "tok-oauth"),
        ]);
        // first configured var wins -> OAUTH_TOKEN is listed first.
        assert_eq!(
            get_env_api_key("anthropic", Some(&env)).as_deref(),
            Some("tok-oauth")
        );
    }

    #[test]
    fn bedrock_ambient_credentials_sentinel() {
        // Pinned against an EMPTY ambient tier, so "missing secret" is a property of the fixture.
        // Read against the real process env this asserted a property of the host: any machine with
        // an ambient `AWS_SECRET_ACCESS_KEY` (every developer with AWS creds, and this project's
        // own CI container) completed the IAM pair through the `||` fallback and got the sentinel
        // where the test demanded `None`.
        let none = env_of(&[]);
        let ambient = Ambient::Fixed(&none);

        let env = env_of(&[("AWS_PROFILE", "default")]);
        assert_eq!(
            get_env_api_key_in("amazon-bedrock", Some(&env), ambient).as_deref(),
            Some("<authenticated>")
        );
        let env = env_of(&[("AWS_ACCESS_KEY_ID", "id")]); // missing secret → not authenticated
        assert_eq!(
            get_env_api_key_in("amazon-bedrock", Some(&env), ambient),
            None
        );
        // …and the pair completed is authenticated, which the old shape could not distinguish
        // from the ambient leak above.
        let env = env_of(&[
            ("AWS_ACCESS_KEY_ID", "id"),
            ("AWS_SECRET_ACCESS_KEY", "sec"),
        ]);
        assert_eq!(
            get_env_api_key_in("amazon-bedrock", Some(&env), ambient).as_deref(),
            Some("<authenticated>")
        );
    }

    /// The ambient tier is consulted only when the overlay does not supply the value, and an
    /// injected ambient behaves exactly like the process env would (Pi `env?.[n] || process.env[n]`).
    #[test]
    fn overlay_wins_over_ambient_and_empty_overlay_defers_to_it() {
        let ambient_map = env_of(&[("OPENAI_API_KEY", "sk-ambient")]);
        let ambient = Ambient::Fixed(&ambient_map);

        // Overlay present and non-empty → overlay wins.
        let overlay = env_of(&[("OPENAI_API_KEY", "sk-overlay")]);
        assert_eq!(
            get_env_api_key_in("openai", Some(&overlay), ambient).as_deref(),
            Some("sk-overlay")
        );
        // Overlay empty-valued → falls through to ambient (JS `||` skips empty strings).
        let overlay = env_of(&[("OPENAI_API_KEY", "")]);
        assert_eq!(
            get_env_api_key_in("openai", Some(&overlay), ambient).as_deref(),
            Some("sk-ambient")
        );
        // Nothing anywhere → genuinely unset, now assertable without consulting the host.
        let empty = env_of(&[]);
        assert_eq!(
            get_env_api_key_in("openai", Some(&empty), Ambient::Fixed(&empty)),
            None
        );
    }
}
