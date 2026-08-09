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

use crate::HeaderMap;
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
use crate::stream::StreamOptions;
use crate::stream::sse::{SseRequest, build_client_for_target, open_sse};
use crate::utils::provider_retry::ProviderRetry;
use cyrup_core::{ApiId, CancelToken, ModelThinkingLevel};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = "azure-openai-responses";

/// The default Azure OpenAI API version (Pi `DEFAULT_AZURE_API_VERSION`,
/// azure-openai-responses.ts:19).
const DEFAULT_AZURE_API_VERSION: &str = "v1";

/// Per-API typed options for the `azure-openai-responses` wire protocol (Pi
/// `AzureOpenAIResponsesOptions`, azure-openai-responses.ts:52-59). Each per-request override wins
/// over the corresponding `AZURE_OPENAI_*` provider-env value. Carried via
/// [`StreamOptions::api_options`](crate::StreamOptions::api_options); all fields default to `None`
/// (env-only resolution, unchanged behavior).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AzureOpenAiResponsesOptions {
    /// Pi `azureApiVersion` (azure-openai-responses.ts:55) — wins over `AZURE_OPENAI_API_VERSION`.
    pub azure_api_version: Option<String>,
    /// Pi `azureResourceName` (azure-openai-responses.ts:56) — wins over `AZURE_OPENAI_RESOURCE_NAME`.
    pub azure_resource_name: Option<String>,
    /// Pi `azureBaseUrl` (azure-openai-responses.ts:57) — wins over `AZURE_OPENAI_BASE_URL`.
    pub azure_base_url: Option<String>,
    /// Pi `azureDeploymentName` (azure-openai-responses.ts:58) — wins over the deployment-name map.
    pub azure_deployment_name: Option<String>,
}

/// Providers whose tool-call ids carry the `call_id|item_id` Responses shape (Pi
/// `AZURE_TOOL_CALL_PROVIDERS`, azure-openai-responses.ts:20).
const AZURE_TOOL_CALL_PROVIDERS: &[&str] = &[
    "openai",
    "openai-codex",
    "opencode",
    "azure-openai-responses",
];

/// The `ApiImpl` for `"azure-openai-responses"`.
pub struct AzureOpenAiResponsesApi {
    api: ApiId,
}

impl Default for AzureOpenAiResponsesApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
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
            let e = ProviderError::Transport(format!("No API key for provider: {provider}").into());
            sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                .await;
            return;
        };

        let azure = opts.azure_openai_responses_options();
        // resolveAzureConfig → base URL + api version (option override → env), then `/responses`.
        let url = match resolve_request_url(model, env, azure) {
            Ok(url) => url,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        let deployment = resolve_deployment_name(model, env, azure);
        // gap-08 #2: `before_provider_request` may inspect/replace the outbound body.
        let body = crate::stream::apply_on_payload(
            opts,
            model,
            build_params(model, ctx, opts, &deployment),
        )
        .await;
        let headers = build_headers(model, opts, &api_key);
        let req = SseRequest {
            method: reqwest::Method::POST,
            url,
            headers,
            body: Some(body),
        };

        // Honor HTTP(S)_PROXY for the live client (Pi resolveHttpProxyUrlForTarget,
        // node-http-proxy.ts:92-112).
        // PROV-006: the request idle timeout. `StreamOptions.timeout_ms` overrides the
        // process-global `configure_http_idle_timeout` default, exactly as Pi layers the SDK
        // client's `timeout` on top of the global undici dispatcher (sdk.ts:304-309).
        let client = match build_client_for_target(
            &req.url,
            &crate::auth::types::EnvAuthContext,
            auth.env.as_ref(),
            opts.timeout_ms,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };

        // gap-08 #3: capture {status, headers} at connect, then fire `after_provider_response`.
        let capture = crate::stream::ResponseCapture::default();
        let on_resp = capture.sse_hook(opts);
        let frames = match open_sse(
            &client,
            req,
            cancel,
            None,
            on_resp,
            ProviderRetry::from_options(opts),
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }
        };
        capture.fire(opts, model).await;

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

/// 1:1 port of Pi `resolveDeploymentName` (azure-openai-responses.ts:37-44): the per-request
/// `azureDeploymentName` override wins; else the `AZURE_OPENAI_DEPLOYMENT_NAME_MAP` mapping for this
/// model id, else the model id itself.
fn resolve_deployment_name(
    model: &Model,
    env: Option<&ProviderEnv>,
    azure: Option<&AzureOpenAiResponsesOptions>,
) -> String {
    if let Some(name) = azure
        .and_then(|o| o.azure_deployment_name.as_deref())
        .filter(|s| !s.is_empty())
    {
        return name.to_string();
    }
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

/// 1:1 port of Pi `resolveAzureConfig` (azure-openai-responses.ts:198-249): resolve the
/// (normalized base URL, api version). Each per-request override (`azureApiVersion`/`azureBaseUrl`/
/// `azureResourceName`) wins over its `AZURE_OPENAI_*` env value; the base URL then falls back to
/// `AZURE_OPENAI_RESOURCE_NAME`/`azureResourceName` → `model.baseUrl`, erroring when none is set.
fn resolve_azure_config(
    model: &Model,
    env: Option<&ProviderEnv>,
    azure: Option<&AzureOpenAiResponsesOptions>,
) -> Result<(String, String), ProviderError> {
    // `options?.azureApiVersion || env || DEFAULT` (empty string is falsy in Pi's `||`).
    let api_version = azure
        .and_then(|o| o.azure_api_version.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| provider_env_value("AZURE_OPENAI_API_VERSION", env))
        .unwrap_or_else(|| DEFAULT_AZURE_API_VERSION.to_string());

    // `options?.azureBaseUrl?.trim() || env?.trim() || undefined`.
    let mut resolved = azure
        .and_then(|o| o.azure_base_url.as_ref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            provider_env_value("AZURE_OPENAI_BASE_URL", env)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });

    // `options?.azureResourceName || env`.
    let resource_name = azure
        .and_then(|o| o.azure_resource_name.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| provider_env_value("AZURE_OPENAI_RESOURCE_NAME", env));

    if resolved.is_none()
        && let Some(resource) = resource_name.filter(|s| !s.is_empty())
    {
        resolved = Some(build_default_base_url(&resource));
    }

    if resolved.is_none()
        && let Some(base) = Some(model.base_url.as_str()).filter(|s| !s.is_empty())
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
fn resolve_request_url(
    model: &Model,
    env: Option<&ProviderEnv>,
    azure: Option<&AzureOpenAiResponsesOptions>,
) -> Result<String, ProviderError> {
    let (base, api_version) = resolve_azure_config(model, env, azure)?;
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
    // Azure gets NO deferred-tool loading: Pi's `azure-openai-responses.ts:280` calls
    // `convertResponsesMessages` with options that omit `deferredTools` and never imports
    // `splitDeferredTools`, so no `tool_search_call`/`tool_search_output` pair can ever be emitted
    // on this path and every tool stays in the request prefix (DRIFT-001).
    let messages = convert_responses_messages(model, ctx, AZURE_TOOL_CALL_PROVIDERS, &[]);

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
        obj.insert(
            "tools".to_string(),
            Value::Array(convert_responses_tools(&ctx.tools, false)),
        );
    }

    if model.reasoning {
        // The unified reasoning level maps to Pi's `reasoningEffort` (clamped; `off` => the
        // thinkingLevelMap.off branch). `reasoningSummary` is typed-options-only (gap #11), so the
        // summary defaults to "auto" on the simple/env path.
        let clamped = clamp_thinking_level(model, opts.reasoning);
        if clamped != ModelThinkingLevel::Off {
            let key = thinking_level_key(clamped);
            let effort = mapped_effort_or(model.thinking_level_map.as_ref(), clamped, key);
            obj.insert(
                "reasoning".to_string(),
                json!({ "effort": effort, "summary": "auto" }),
            );
            obj.insert(
                "include".to_string(),
                json!(["reasoning.encrypted_content"]),
            );
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
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
            base_url: String::new(),
            reasoning,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window: 1000,
            max_tokens: 1000,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn env(pairs: &[(&str, &str)]) -> ProviderEnv {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn deployment_map_parses_and_resolves() {
        let map = parse_deployment_name_map(Some("gpt-4=dep-a, gpt-5 = dep-b ,,bad=,=nope"));
        assert_eq!(map.get("gpt-4").map(String::as_str), Some("dep-a"));
        assert_eq!(map.get("gpt-5").map(String::as_str), Some("dep-b"));
        assert_eq!(map.len(), 2); // blank, `bad=`, and `=nope` skipped

        let m = azure_model("gpt-4", false);
        let e = env(&[("AZURE_OPENAI_DEPLOYMENT_NAME_MAP", "gpt-4=my-deploy")]);
        assert_eq!(resolve_deployment_name(&m, Some(&e), None), "my-deploy");
        // Unmapped model id falls back to the id itself.
        let m2 = azure_model("gpt-5", false);
        assert_eq!(resolve_deployment_name(&m2, Some(&e), None), "gpt-5");

        // Per-request `azureDeploymentName` override wins over the env map (Pi line 38-39).
        let opt = AzureOpenAiResponsesOptions {
            azure_deployment_name: Some("override-dep".to_string()),
            ..Default::default()
        };
        assert_eq!(
            resolve_deployment_name(&m, Some(&e), Some(&opt)),
            "override-dep"
        );
        // An empty-string override is falsy (Pi `if (options?.azureDeploymentName)`) and falls back
        // to the env map.
        let empty = AzureOpenAiResponsesOptions {
            azure_deployment_name: Some(String::new()),
            ..Default::default()
        };
        assert_eq!(
            resolve_deployment_name(&m, Some(&e), Some(&empty)),
            "my-deploy"
        );
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
            normalize_azure_base_url(
                "https://my-res.cognitiveservices.azure.com/openai/v1/responses"
            )
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
        let (base, ver) = resolve_azure_config(&m, Some(&e), None).unwrap();
        assert_eq!(base, "https://b.openai.azure.com/openai/v1");
        assert_eq!(ver, "2025-01-01");

        // Resource name builds the default base; default api version.
        let e2 = env(&[("AZURE_OPENAI_RESOURCE_NAME", "myres")]);
        let (base2, ver2) = resolve_azure_config(&m, Some(&e2), None).unwrap();
        assert_eq!(base2, "https://myres.openai.azure.com/openai/v1");
        assert_eq!(ver2, DEFAULT_AZURE_API_VERSION);

        // Nothing configured (model.baseUrl is empty) → error.
        assert!(resolve_azure_config(&m, Some(&env(&[])), None).is_err());
    }

    #[test]
    fn resolve_config_per_request_overrides_win_over_env() {
        // Each per-request override wins over its AZURE_OPENAI_* env value (Pi azure-openai-
        // responses.ts:202-208).
        let m = azure_model("gpt-4", false);
        let e = env(&[
            ("AZURE_OPENAI_BASE_URL", "https://env.openai.azure.com"),
            ("AZURE_OPENAI_API_VERSION", "2024-env"),
        ]);
        let opt = AzureOpenAiResponsesOptions {
            azure_base_url: Some("https://opt.openai.azure.com".to_string()),
            azure_api_version: Some("2025-opt".to_string()),
            ..Default::default()
        };
        let (base, ver) = resolve_azure_config(&m, Some(&e), Some(&opt)).unwrap();
        assert_eq!(base, "https://opt.openai.azure.com/openai/v1");
        assert_eq!(ver, "2025-opt");

        // azureResourceName override builds the default base when no base URL is set anywhere.
        let opt2 = AzureOpenAiResponsesOptions {
            azure_resource_name: Some("optres".to_string()),
            ..Default::default()
        };
        let (base2, _) = resolve_azure_config(&m, Some(&env(&[])), Some(&opt2)).unwrap();
        assert_eq!(base2, "https://optres.openai.azure.com/openai/v1");
    }

    #[test]
    fn request_url_appends_api_version() {
        let m = azure_model("gpt-4", false);
        let e = env(&[("AZURE_OPENAI_RESOURCE_NAME", "res")]);
        assert_eq!(
            resolve_request_url(&m, Some(&e), None).unwrap(),
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
        assert_eq!(
            body.get("model").and_then(Value::as_str),
            Some("my-deployment")
        );
        assert_eq!(body.get("store").and_then(Value::as_bool), Some(false));
        assert_eq!(
            body.get("prompt_cache_key").and_then(Value::as_str),
            Some("sess-1")
        );
        // Azure never sets prompt_cache_retention.
        assert!(body.get("prompt_cache_retention").is_none());
        assert_eq!(
            body.get("max_output_tokens").and_then(Value::as_u64),
            Some(128)
        );
    }

    #[test]
    fn build_params_reasoning_effort_and_include() {
        let mut m = azure_model("o5", true);
        m.thinking_level_map = Some(
            [("high".to_string(), Some("high".to_string()))]
                .into_iter()
                .collect(),
        );
        let opts = StreamOptions {
            reasoning: ModelThinkingLevel::High,
            ..Default::default()
        };
        let body = build_params(&m, &Context::default(), &opts, "o5");
        let reasoning = body.get("reasoning").expect("reasoning");
        assert_eq!(
            reasoning.get("effort").and_then(Value::as_str),
            Some("high")
        );
        assert_eq!(
            reasoning.get("summary").and_then(Value::as_str),
            Some("auto")
        );
        assert_eq!(
            body.get("include")
                .and_then(Value::as_array)
                .map(|a| a.len()),
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

    #[test]
    fn azure_never_defers_tools_even_with_the_compat_flag_on() {
        // DRIFT-001: Pi's `azure-openai-responses.ts:280` calls `convertResponsesMessages` WITHOUT
        // `deferredTools` and never imports `splitDeferredTools`, so this path cannot emit a
        // `tool_search_call`/`tool_search_output` pair and every tool stays in `body.tools` —
        // even when `compat.supportsToolSearch` is set, which on this api is simply not read.
        let mut m = azure_model("gpt-4", false);
        m.compat = Some(crate::api::compat::ModelCompat {
            supports_tool_search: Some(true),
            ..Default::default()
        });
        let tools = vec![
            crate::context::ToolDef {
                name: "base_tool".into(),
                description: "The base_tool tool".into(),
                parameters: json!({ "type": "object" }),
            },
            crate::context::ToolDef {
                name: "late_tool".into(),
                description: "The late_tool tool".into(),
                parameters: json!({ "type": "object" }),
            },
        ];
        let ctx = Context {
            system_prompt: None,
            messages: vec![
                cyrup_core::Message::Assistant(cyrup_core::AssistantMessage {
                    content: vec![cyrup_core::Content::ToolCall(cyrup_core::ToolCall {
                        id: cyrup_core::ToolCallId::from("call_1"),
                        name: "base_tool".to_string(),
                        arguments: serde_json::Map::new(),
                        thought_signature: None,
                    })],
                    provider: "azure-openai-responses".into(),
                    model: "gpt-4".to_string(),
                    api: API_ID.into(),
                    response_model: None,
                    response_id: None,
                    diagnostics: None,
                    usage: cyrup_core::Usage::default(),
                    stop_reason: cyrup_core::StopReason::ToolUse,
                    deferred: None,
                    error_message: None,
                    raw_stop_reason: None,
                    timestamp: 2,
                }),
                cyrup_core::Message::ToolResult {
                    tool_call_id: cyrup_core::ToolCallId::from("call_1"),
                    tool_name: "base_tool".to_string(),
                    content: vec![cyrup_core::Content::text("done")],
                    is_error: false,
                    details: None,
                    usage: None,
                    added_tool_names: vec!["late_tool".to_string()],
                    timestamp: 3,
                },
            ],
            tools,
        };

        let body = build_params(&m, &ctx, &StreamOptions::default(), "dep");
        let names: Vec<&str> = body["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["base_tool", "late_tool"]);
        let raw = serde_json::to_string(&body).expect("serialize");
        assert!(
            !raw.contains("tool_search"),
            "azure emitted tool search: {raw}"
        );
        assert!(
            !raw.contains("defer_loading"),
            "azure emitted defer_loading: {raw}"
        );
    }
}
