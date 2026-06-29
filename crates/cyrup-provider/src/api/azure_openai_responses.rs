//! The `azure-openai-responses` wire protocol (arch-01 §3.4). A variant of
//! [`openai-responses`](crate::api::openai_responses) that targets the Azure OpenAI Responses
//! deployment surface: it reuses the shared Responses message/tool encoders and the full streaming
//! decoder, but diverges in (a) the request URL (`{normalizedBaseUrl}/responses?api-version=<ver>`
//! with Azure host base-path normalization), (b) auth (the `api-key:` header, not
//! `Authorization: Bearer`), (c) the `model` field carrying a resolved *deployment* name, and (d) a
//! distinct `buildParams` (always-on `prompt_cache_key`, no `prompt_cache_retention`).
//!
//! 1:1 port of Pi's `api/azure-openai-responses.ts` (`resolveDeploymentName`,
//! `parseDeploymentNameMap`, `normalizeAzureBaseUrl`, `buildDefaultBaseUrl`, `resolveAzureConfig`,
//! `createClient`, `buildParams`) over the shared `openai-responses-shared.ts` encoder/decoder.
//!
//! Azure configuration is resolved from the provider env (`AZURE_OPENAI_API_VERSION`,
//! `AZURE_OPENAI_BASE_URL`, `AZURE_OPENAI_RESOURCE_NAME`, `AZURE_OPENAI_DEPLOYMENT_NAME_MAP`) — the
//! `streamSimple` lowering path the [`crate::wire::WireProvider`] drives. The typed per-request
//! `azureApiVersion`/`azureResourceName`/`azureBaseUrl`/`azureDeploymentName`/`reasoningSummary`
//! overrides (Pi `AzureOpenAIResponsesOptions`) are part of the typed per-API options downcast
//! surface (gap #11) and are not reachable through the unified `StreamOptions`.

use crate::api::compat::{
    clamp_openai_prompt_cache_key, mapped_effort_or, off_is_not_null, off_value_or,
    thinking_level_key,
};
use crate::api::openai_responses::{
    convert_responses_messages, convert_responses_tools, decode_stream, provider_env_value,
};
use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::auth::ProviderEnv;
use crate::collection::clamp_thinking_level;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::sse::{build_client, open_sse, SseRequest};
use crate::stream::StreamOptions;
use crate::HeaderMap;
use cyrup_core::{ApiId, CancelToken, ModelThinkingLevel};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = "azure-openai-responses";

/// The default Azure OpenAI API version (Pi `DEFAULT_AZURE_API_VERSION`,
/// azure-openai-responses.ts:19).
const DEFAULT_AZURE_API_VERSION: &str = "v1";

/// Providers whose tool-call ids carry the `call_id|item_id` Responses shape (Pi
/// `AZURE_TOOL_CALL_PROVIDERS`, azure-openai-responses.ts:20).
const AZURE_TOOL_CALL_PROVIDERS: &[&str] =
    &["openai", "openai-codex", "opencode", "azure-openai-responses"];

/// The `ApiImpl` for `"azure-openai-responses"`.
pub struct AzureOpenAiResponsesApi {
    api: ApiId,
}

impl Default for AzureOpenAiResponsesApi {
    fn default() -> Self {
        Self { api: ApiId::from(API_ID) }
    }
}

impl AzureOpenAiResponsesApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(AzureOpenAiResponsesApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for AzureOpenAiResponsesApi {
    fn api(&self) -> &ApiId {
        &self.api
    }

    async fn run(
        &self,
        model: &Model,
        ctx: &Context,
        auth: &AuthResult,
        opts: &StreamOptions,
        cancel: CancelToken,
        sink: EventSink,
    ) {
        let provider = model.provider.clone();
        let model_id = model.id.as_str().to_string();
        let env = auth.env.as_ref();

        // No api key → Pi `throw new Error("No API key for provider: …")`.
        let Some(api_key) = auth.auth.api_key.clone() else {
            let e =
                ProviderError::Transport(format!("No API key for provider: {provider}").into());
            sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone()))).await;
            return;
        };

        // resolveAzureConfig → base URL + api version (env-driven), then the `/responses` endpoint.
        let url = match resolve_request_url(model, env) {
            Ok(url) => url,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone()))).await;
                return;
            }
        };

        let deployment = resolve_deployment_name(model, env);
        let body = build_params(model, ctx, opts, &deployment);
        let headers = build_headers(model, opts, &api_key);
        let req = SseRequest { method: reqwest::Method::POST, url, headers, body: Some(body) };

        let client = match build_client() {
            Ok(c) => c,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone()))).await;
                return;
            }
        };

        let frames = match open_sse(&client, req, cancel, None, None).await {
            Ok(s) => s,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone()))).await;
                return;
            }
        };

        // Azure speaks the identical Responses SSE wire format → reuse the shared decoder.
        decode_stream(frames, model, &self.api, &sink).await;
    }
}

// ---------------------------------------------------------------------------
// Azure configuration (Pi azure-openai-responses.ts)
// ---------------------------------------------------------------------------

/// 1:1 port of Pi `parseDeploymentNameMap` (azure-openai-responses.ts:22-33): parse a
/// `modelId=deployment,modelId2=deployment2` map, skipping blank/half entries.
fn parse_deployment_name_map(value: Option<&str>) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Some(value) = value else { return map };
    for entry in value.split(',') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, '=');
        let model_id = parts.next().unwrap_or("").trim();
        let deployment = parts.next().unwrap_or("").trim();
        if model_id.is_empty() || deployment.is_empty() {
            continue;
        }
        map.insert(model_id.to_string(), deployment.to_string());
    }
    map
}

/// 1:1 port of Pi `resolveDeploymentName` (azure-openai-responses.ts:35-43): the
/// `AZURE_OPENAI_DEPLOYMENT_NAME_MAP` mapping for this model id, else the model id itself. (The
/// per-request `azureDeploymentName` override is part of the typed-options surface, gap #11.)
fn resolve_deployment_name(model: &Model, env: Option<&ProviderEnv>) -> String {
    let map = parse_deployment_name_map(
        provider_env_value("AZURE_OPENAI_DEPLOYMENT_NAME_MAP", env).as_deref(),
    );
    map.get(model.id.as_str())
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| model.id.as_str().to_string())
}

/// 1:1 port of Pi `buildDefaultBaseUrl` (azure-openai-responses.ts:215-217).
fn build_default_base_url(resource_name: &str) -> String {
    format!("https://{resource_name}.openai.azure.com/openai/v1")
}

/// 1:1 port of Pi `normalizeAzureBaseUrl` (azure-openai-responses.ts:170-204): trim trailing
/// slashes, validate the URL, and for Azure hosts coerce a bare/`/openai`/`/openai/v1/responses`
/// path to `/openai/v1` (clearing any query) so `{base}/responses?api-version=…` is well-formed.
fn normalize_azure_base_url(base_url: &str) -> Result<String, ProviderError> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let mut url = reqwest::Url::parse(trimmed).map_err(|_| {
        ProviderError::Transport(format!("Invalid Azure OpenAI base URL: {base_url}").into())
    })?;

    let host = url.host_str().unwrap_or("");
    let is_azure_host = host.ends_with(".openai.azure.com")
        || host.ends_with(".cognitiveservices.azure.com")
        || host.ends_with(".ai.azure.com");
    let normalized_path = url.path().trim_end_matches('/');

    if is_azure_host
        && (normalized_path.is_empty()
            || normalized_path == "/openai"
            || normalized_path == "/openai/v1/responses")
    {
        url.set_path("/openai/v1");
        url.set_query(None);
    }

    Ok(url.to_string().trim_end_matches('/').to_string())
}

/// 1:1 port of Pi `resolveAzureConfig` (azure-openai-responses.ts:206-249): resolve the
/// (normalized base URL, api version) from `AZURE_OPENAI_BASE_URL` → `AZURE_OPENAI_RESOURCE_NAME` →
/// `model.baseUrl`, erroring when none is configured.
fn resolve_azure_config(
    model: &Model,
    env: Option<&ProviderEnv>,
) -> Result<(String, String), ProviderError> {
    let api_version = provider_env_value("AZURE_OPENAI_API_VERSION", env)
        .unwrap_or_else(|| DEFAULT_AZURE_API_VERSION.to_string());

    let mut resolved = provider_env_value("AZURE_OPENAI_BASE_URL", env)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if resolved.is_none()
        && let Some(resource) = provider_env_value("AZURE_OPENAI_RESOURCE_NAME", env)
            .filter(|s| !s.is_empty())
    {
        resolved = Some(build_default_base_url(&resource));
    }

    if resolved.is_none()
        && let Some(base) = model.base_url.as_deref().filter(|s| !s.is_empty())
    {
        resolved = Some(base.to_string());
    }

    let Some(resolved) = resolved else {
        return Err(ProviderError::Transport(
            "Azure OpenAI base URL is required. Set AZURE_OPENAI_BASE_URL or \
             AZURE_OPENAI_RESOURCE_NAME, or pass azureBaseUrl, azureResourceName, or model.baseUrl."
                .into(),
        ));
    };

    Ok((normalize_azure_base_url(&resolved)?, api_version))
}

/// The `POST` target: `{normalizedBaseUrl}/responses?api-version=<ver>` (the AzureOpenAI SDK's
/// `responses.create` route on the `/openai/v1` base).
fn resolve_request_url(model: &Model, env: Option<&ProviderEnv>) -> Result<String, ProviderError> {
    let (base, api_version) = resolve_azure_config(model, env)?;
    Ok(format!("{base}/responses?api-version={api_version}"))
}

// ---------------------------------------------------------------------------
// Request encoding
// ---------------------------------------------------------------------------

/// 1:1 port of Pi `buildParams` (azure-openai-responses.ts:251-307). Differs from
/// `openai-responses` `buildParams`: the `model` field carries the resolved *deployment* name, the
/// `prompt_cache_key` is set whenever a session id is present (no cache-retention gate), and there
/// is no `prompt_cache_retention`.
pub(crate) fn build_params(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    deployment_name: &str,
) -> Value {
    let messages = convert_responses_messages(model, ctx, AZURE_TOOL_CALL_PROVIDERS);

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(deployment_name));
    obj.insert("input".to_string(), Value::Array(messages));
    obj.insert("stream".to_string(), json!(true));

    // prompt_cache_key: clampOpenAIPromptCacheKey(sessionId) — `undefined` (omitted) when absent.
    if let Some(sid) = &opts.session_id {
        obj.insert(
            "prompt_cache_key".to_string(),
            json!(clamp_openai_prompt_cache_key(sid.as_str())),
        );
    }
    obj.insert("store".to_string(), json!(false));

    if let Some(max) = opts.max_tokens {
        obj.insert("max_output_tokens".to_string(), json!(max));
    }
    if let Some(temp) = opts.temperature {
        obj.insert("temperature".to_string(), json!(temp));
    }

    if !ctx.tools.is_empty() {
        obj.insert("tools".to_string(), Value::Array(convert_responses_tools(&ctx.tools)));
    }

    if model.reasoning {
        // The unified reasoning level maps to Pi's `reasoningEffort` (clamped; `off` => the
        // thinkingLevelMap.off branch). `reasoningSummary` is typed-options-only (gap #11), so the
        // summary defaults to "auto" on the simple/env path.
        let clamped = clamp_thinking_level(model, opts.reasoning);
        if clamped != ModelThinkingLevel::Off {
            let key = thinking_level_key(clamped);
            let effort = mapped_effort_or(model.thinking_level_map.as_ref(), clamped, key);
            obj.insert("reasoning".to_string(), json!({ "effort": effort, "summary": "auto" }));
            obj.insert("include".to_string(), json!(["reasoning.encrypted_content"]));
        } else if off_is_not_null(model.thinking_level_map.as_ref()) {
            let effort = off_value_or(model.thinking_level_map.as_ref(), "none");
            obj.insert("reasoning".to_string(), json!({ "effort": effort }));
        }
    }

    Value::Object(obj)
}

/// Build the request headers (Pi `createClient`, azure-openai-responses.ts:282-308): the AzureOpenAI
/// SDK authenticates with the `api-key` header (not `Authorization: Bearer`), then merges
/// `{ ...model.headers }` and the per-request `opts.headers` overlay (a `None` value suppresses a
/// default).
fn build_headers(model: &Model, opts: &StreamOptions, api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("api-key".to_string(), Some(api_key.to_string()));

    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    headers
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::{Modality, ModelCost};
    use cyrup_core::SessionId;

    fn azure_model(id: &str, reasoning: bool) -> Model {
        Model {
            id: id.into(),
            name: "M".into(),
            api: API_ID.into(),
            provider: "azure-openai-responses".into(),
            base_url: Some(String::new()),
            reasoning,
            input: vec![Modality::Text],
            output: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 1000,
            max_tokens: 1000,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn env(pairs: &[(&str, &str)]) -> ProviderEnv {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn deployment_map_parses_and_resolves() {
        let map = parse_deployment_name_map(Some("gpt-4=dep-a, gpt-5 = dep-b ,,bad=,=nope"));
        assert_eq!(map.get("gpt-4").map(String::as_str), Some("dep-a"));
        assert_eq!(map.get("gpt-5").map(String::as_str), Some("dep-b"));
        assert_eq!(map.len(), 2); // blank, `bad=`, and `=nope` skipped

        let m = azure_model("gpt-4", false);
        let e = env(&[("AZURE_OPENAI_DEPLOYMENT_NAME_MAP", "gpt-4=my-deploy")]);
        assert_eq!(resolve_deployment_name(&m, Some(&e)), "my-deploy");
        // Unmapped model id falls back to the id itself.
        let m2 = azure_model("gpt-5", false);
        assert_eq!(resolve_deployment_name(&m2, Some(&e)), "gpt-5");
    }

    #[test]
    fn normalize_azure_host_paths() {
        // Bare azure host → /openai/v1.
        assert_eq!(
            normalize_azure_base_url("https://my-res.openai.azure.com").unwrap(),
            "https://my-res.openai.azure.com/openai/v1"
        );
        // /openai → /openai/v1.
        assert_eq!(
            normalize_azure_base_url("https://my-res.openai.azure.com/openai/").unwrap(),
            "https://my-res.openai.azure.com/openai/v1"
        );
        // Full /openai/v1/responses path coerced back to the base.
        assert_eq!(
            normalize_azure_base_url("https://my-res.cognitiveservices.azure.com/openai/v1/responses")
                .unwrap(),
            "https://my-res.cognitiveservices.azure.com/openai/v1"
        );
        // Non-azure host: left as-is (trailing slash trimmed).
        assert_eq!(
            normalize_azure_base_url("https://proxy.example.com/openai/v1/").unwrap(),
            "https://proxy.example.com/openai/v1"
        );
        // Invalid URL → error.
        assert!(normalize_azure_base_url("not a url").is_err());
    }

    #[test]
    fn resolve_config_precedence_base_then_resource() {
        let m = azure_model("gpt-4", false);
        // Explicit base URL wins.
        let e = env(&[
            ("AZURE_OPENAI_BASE_URL", "https://b.openai.azure.com"),
            ("AZURE_OPENAI_RESOURCE_NAME", "res"),
            ("AZURE_OPENAI_API_VERSION", "2025-01-01"),
        ]);
        let (base, ver) = resolve_azure_config(&m, Some(&e)).unwrap();
        assert_eq!(base, "https://b.openai.azure.com/openai/v1");
        assert_eq!(ver, "2025-01-01");

        // Resource name builds the default base; default api version.
        let e2 = env(&[("AZURE_OPENAI_RESOURCE_NAME", "myres")]);
        let (base2, ver2) = resolve_azure_config(&m, Some(&e2)).unwrap();
        assert_eq!(base2, "https://myres.openai.azure.com/openai/v1");
        assert_eq!(ver2, DEFAULT_AZURE_API_VERSION);

        // Nothing configured (model.baseUrl is empty) → error.
        assert!(resolve_azure_config(&m, Some(&env(&[]))).is_err());
    }

    #[test]
    fn request_url_appends_api_version() {
        let m = azure_model("gpt-4", false);
        let e = env(&[("AZURE_OPENAI_RESOURCE_NAME", "res")]);
        assert_eq!(
            resolve_request_url(&m, Some(&e)).unwrap(),
            "https://res.openai.azure.com/openai/v1/responses?api-version=v1"
        );
    }

    #[test]
    fn build_params_uses_deployment_and_always_caches_with_session() {
        let m = azure_model("gpt-4", false);
        let ctx = Context::default();
        let opts = StreamOptions {
            session_id: Some(SessionId::from("sess-1")),
            max_tokens: Some(128),
            ..Default::default()
        };
        let body = build_params(&m, &ctx, &opts, "my-deployment");
        assert_eq!(body.get("model").and_then(Value::as_str), Some("my-deployment"));
        assert_eq!(body.get("store").and_then(Value::as_bool), Some(false));
        assert_eq!(body.get("prompt_cache_key").and_then(Value::as_str), Some("sess-1"));
        // Azure never sets prompt_cache_retention.
        assert!(body.get("prompt_cache_retention").is_none());
        assert_eq!(body.get("max_output_tokens").and_then(Value::as_u64), Some(128));
    }

    #[test]
    fn build_params_reasoning_effort_and_include() {
        let mut m = azure_model("o5", true);
        m.thinking_level_map = Some(
            [("high".to_string(), Some("high".to_string()))].into_iter().collect(),
        );
        let opts =
            StreamOptions { reasoning: ModelThinkingLevel::High, ..Default::default() };
        let body = build_params(&m, &Context::default(), &opts, "o5");
        let reasoning = body.get("reasoning").expect("reasoning");
        assert_eq!(reasoning.get("effort").and_then(Value::as_str), Some("high"));
        assert_eq!(reasoning.get("summary").and_then(Value::as_str), Some("auto"));
        assert_eq!(
            body.get("include").and_then(Value::as_array).map(|a| a.len()),
            Some(1)
        );
    }

    #[test]
    fn build_headers_uses_api_key_not_bearer() {
        let m = azure_model("gpt-4", false);
        let h = build_headers(&m, &StreamOptions::default(), "azkey");
        assert_eq!(h.get("api-key"), Some(&Some("azkey".to_string())));
        assert!(!h.contains_key("Authorization"));
    }
}
