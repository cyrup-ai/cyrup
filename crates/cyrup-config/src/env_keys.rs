//! Provider → environment-variable mapping (Pi `ai/src/env-api-keys.ts`).
//!
//! Ports `getApiKeyEnvVars` / `findEnvKeys` / `getEnvApiKey` (env-api-keys.ts:64-177): the canonical
//! per-provider API-key env-var names plus the Vertex-ADC / Bedrock ambient-credential probes. This
//! feeds the `env` tier of [`crate::auth::resolve_auth`] and the `getAuthStatus` label.

use std::collections::HashMap;

/// Lookup a provider-scoped value, falling back to the process environment, treating empty as unset
/// (Pi `getProviderEnvValue`). `env` is an optional override map (used in tests / scoped configs).
fn provider_env_value(name: &str, env: Option<&HashMap<String, String>>) -> Option<String> {
    if let Some(map) = env
        && let Some(v) = map.get(name)
        && !v.is_empty()
    {
        return Some(v.clone());
    }
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
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
        "openai" => &["OPENAI_API_KEY"],
        "azure-openai-responses" => &["AZURE_OPENAI_API_KEY"],
        "nvidia" => &["NVIDIA_API_KEY"],
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "google" => &["GEMINI_API_KEY"],
        "google-vertex" => &["GOOGLE_CLOUD_API_KEY"],
        "groq" => &["GROQ_API_KEY"],
        "cerebras" => &["CEREBRAS_API_KEY"],
        "xai" => &["XAI_API_KEY"],
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
    let vars = api_key_env_vars(provider)?;
    let found: Vec<String> =
        vars.iter().filter(|v| provider_env_value(v, env).is_some()).map(|v| v.to_string()).collect();
    if found.is_empty() { None } else { Some(found) }
}

/// Whether the default Vertex ADC credentials file exists (Pi `hasVertexAdcCredentials`, :31-62).
fn has_vertex_adc_credentials(env: Option<&HashMap<String, String>>) -> bool {
    if let Some(explicit) = provider_env_value("GOOGLE_APPLICATION_CREDENTIALS", env) {
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
    if let Some(keys) = find_env_keys(provider, env)
        && let Some(first) = keys.first()
    {
        return provider_env_value(first, env);
    }

    if provider == "google-vertex" {
        let has_credentials = has_vertex_adc_credentials(env);
        let has_project = provider_env_value("GOOGLE_CLOUD_PROJECT", env).is_some()
            || provider_env_value("GCLOUD_PROJECT", env).is_some();
        let has_location = provider_env_value("GOOGLE_CLOUD_LOCATION", env).is_some();
        if has_credentials && has_project && has_location {
            return Some("<authenticated>".to_string());
        }
    }

    if provider == "amazon-bedrock" {
        let has_aws_keys = provider_env_value("AWS_ACCESS_KEY_ID", env).is_some()
            && provider_env_value("AWS_SECRET_ACCESS_KEY", env).is_some();
        if provider_env_value("AWS_PROFILE", env).is_some()
            || has_aws_keys
            || provider_env_value("AWS_BEARER_TOKEN_BEDROCK", env).is_some()
            || provider_env_value("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", env).is_some()
            || provider_env_value("AWS_CONTAINER_CREDENTIALS_FULL_URI", env).is_some()
            || provider_env_value("AWS_WEB_IDENTITY_TOKEN_FILE", env).is_some()
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
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn known_provider_env_var_names() {
        assert_eq!(api_key_env_vars("openai"), Some(&["OPENAI_API_KEY"][..]));
        assert_eq!(
            api_key_env_vars("anthropic"),
            Some(&["ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_API_KEY"][..])
        );
        assert_eq!(api_key_env_vars("github-copilot"), Some(&["COPILOT_GITHUB_TOKEN"][..]));
        assert_eq!(api_key_env_vars("totally-unknown-provider"), None);
    }

    #[test]
    fn find_and_get_from_scoped_env_map() {
        let env = env_of(&[("OPENAI_API_KEY", "sk-openai")]);
        assert_eq!(find_env_keys("openai", Some(&env)), Some(vec!["OPENAI_API_KEY".to_string()]));
        assert_eq!(get_env_api_key("openai", Some(&env)).as_deref(), Some("sk-openai"));
        // unset
        let empty = env_of(&[]);
        assert_eq!(find_env_keys("openai", Some(&empty)), None);
        assert_eq!(get_env_api_key("openai", Some(&empty)), None);
    }

    #[test]
    fn anthropic_oauth_token_precedence() {
        let env = env_of(&[("ANTHROPIC_API_KEY", "sk-api"), ("ANTHROPIC_OAUTH_TOKEN", "tok-oauth")]);
        // first configured var wins -> OAUTH_TOKEN is listed first.
        assert_eq!(get_env_api_key("anthropic", Some(&env)).as_deref(), Some("tok-oauth"));
    }

    #[test]
    fn bedrock_ambient_credentials_sentinel() {
        let env = env_of(&[("AWS_PROFILE", "default")]);
        assert_eq!(get_env_api_key("amazon-bedrock", Some(&env)).as_deref(), Some("<authenticated>"));
        let env = env_of(&[("AWS_ACCESS_KEY_ID", "id")]); // missing secret → not authenticated
        assert_eq!(get_env_api_key("amazon-bedrock", Some(&env)), None);
    }
}
