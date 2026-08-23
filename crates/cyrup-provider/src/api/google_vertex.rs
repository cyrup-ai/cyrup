//! The `google-vertex` wire protocol (PROV-030) — a port of pi `packages/ai/src/api/google-vertex.ts`
//! @v0.83.0.
//!
//! Vertex speaks the *same* `GenerateContentRequest` / `GenerateContentResponse` protocol as the
//! Gemini Generative Language API: pi's two adapters share `api/google-shared.ts` and their
//! `buildParams` bodies are line-for-line identical at v0.83.0 (`google-vertex.ts:432-490` vs
//! `google-generative-ai.ts:351-405` — same `convertMessages`, same `generationConfig`, same
//! `resolveGoogleFunctionCallingMode`, same `thinkingConfig` branch). Everything that differs
//! differs *around* the payload:
//!
//! | | `google-generative-ai` | `google-vertex` |
//! |---|---|---|
//! | endpoint | `{base}/models/{id}:streamGenerateContent` | `{base}/v1/projects/{project}/locations/{location}/publishers/google/models/{id}:streamGenerateContent` |
//! | auth | `x-goog-api-key` | `Authorization: Bearer <ADC token>`, or `x-goog-api-key` in express mode |
//! | base url | `model.baseUrl` verbatim | `https://{location}-aiplatform.googleapis.com` with `{location}` interpolated |
//!
//! So this module owns the endpoint, the auth and the option/env plumbing, and delegates the body
//! encoder and the SSE decoder to [`crate::api::google_generative_ai`] rather than cloning 1500
//! lines of converter — which is the same factoring pi performs with `google-shared.ts`, and what
//! PROV-030's own Fix text asks for ("It shares nearly everything with `google_generative_ai.rs`
//! via pi's `api/google-shared.ts`, so factor the shared converters out first").
//!
//! # The two auth arms
//!
//! [`crate::providers::google_vertex::GoogleVertexApiKeyAuth::resolve`] (pi `google-vertex.ts:62-84`)
//! produces one of two shapes, and this module's whole job on the auth axis is to tell them apart
//! the way pi's `stream` does at `google-vertex.ts:98-102`:
//!
//! * **API key present** → pi calls `createClientWithApiKey` (`:349-360`), i.e. Vertex *express
//!   mode*: the location-less global host, no `projects/.../locations/...` path segment, and the
//!   key in `x-goog-api-key`. Guarded by `resolveApiKey` (`:388-394`), which discards the
//!   `gcp-vertex-credentials` marker and `<...>` placeholders.
//! * **No key** → pi calls `createClient` (`:327-339`) with `project`, `location` and
//!   `googleAuthOptions`, and the `@google/genai` SDK mints a bearer through `google-auth-library`.
//!   cyrup has no SDK, so [`crate::auth::google_adc`] reproduces that minting; read that module's
//!   `[CYRUP-DELTA]` for the credential types it accepts.
//!
//! `resolveProject` (`:396-406`) and `resolveLocation` (`:408-414`) are ported verbatim, including
//! their two error strings, because they are the failure a misconfigured user actually sees.
//!
//! # `[CYRUP-DELTA]` — `ProviderError::Transport`'s `"transport error: "` prefix
//!
//! pi throws bare `Error`s here and its catch block runs them through
//! `formatProviderError(normalizeProviderError(error))`, which returns `error.message` unchanged.
//! cyrup routes them through [`ProviderError::Transport`], whose `Display` prefixes
//! `"transport error: "`. That is not a new divergence introduced by this module — it is exactly
//! what the sibling `google_generative_ai.rs:93-98` already does with pi's
//! `No API key for provider: {provider}` throw — but it is stated here so the next reader does not
//! have to re-derive it. The message text itself is byte-identical to pi's.

use crate::HeaderMap;
use crate::api::google_generative_ai::{build_params, decode_stream};
use crate::api::{ApiImpl, EventSink};
use crate::auth::AuthResult;
use crate::auth::types::ProviderEnv;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::StreamOptions;
use crate::stream::sse::{SseRequest, build_client_for_target, open_sse};
use crate::utils::provider_plumbing::provider_env_value;
use crate::utils::provider_retry::ProviderRetry;
use cyrup_core::{ApiId, CancelToken};
use std::sync::Arc;

/// The wire-protocol id this impl serves.
const API_ID: &str = crate::known_api::GOOGLE_VERTEX;

/// pi `API_VERSION` (`google-vertex.ts:54`). Vertex's REST surface is `v1`, not `v1beta`.
pub const API_VERSION: &str = "v1";

/// pi `GCP_VERTEX_CREDENTIALS_MARKER` (`google-vertex.ts:55`). A sentinel some tooling stores in the
/// api-key slot to mean "use ADC"; `resolveApiKey` must treat it as *no key* (`:389-392`).
pub const GCP_VERTEX_CREDENTIALS_MARKER: &str = "gcp-vertex-credentials";

/// The `{location}` placeholder in the catalog's base-URL template
/// ([`crate::providers::google_vertex::GOOGLE_VERTEX_BASE_URL_TEMPLATE`]). pi's
/// `resolveCustomBaseUrl` (`:362-368`) treats a base URL containing it as *not* a custom override,
/// precisely because the SDK is the one that knows the location.
pub const LOCATION_PLACEHOLDER: &str = "{location}";

/// The regional host Vertex uses when `location` is anything but `global`, and the interpolation
/// target of the catalog template.
pub const REGIONAL_HOST_TEMPLATE: &str = "https://{location}-aiplatform.googleapis.com";

/// The location-less host: Vertex's `global` endpoint, and the express-mode (api-key) host.
pub const GLOBAL_HOST: &str = "https://aiplatform.googleapis.com";

/// The `location` value that selects [`GLOBAL_HOST`] instead of a regional host.
pub const GLOBAL_LOCATION: &str = "global";

/// pi `resolveProject`'s throw (`google-vertex.ts:399-403`), verbatim.
pub const MISSING_PROJECT_MESSAGE: &str =
    "Vertex AI requires a project ID. Set GOOGLE_CLOUD_PROJECT/GCLOUD_PROJECT or pass project in options.";

/// pi `resolveLocation`'s throw (`google-vertex.ts:411`), verbatim.
pub const MISSING_LOCATION_MESSAGE: &str =
    "Vertex AI requires a location. Set GOOGLE_CLOUD_LOCATION or pass location in options.";

/// The `ApiImpl` for `"google-vertex"`.
pub struct GoogleVertexApi {
    api: ApiId,
}

impl Default for GoogleVertexApi {
    fn default() -> Self {
        Self {
            api: ApiId::from(API_ID),
        }
    }
}

impl GoogleVertexApi {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Lazy factory for the [`crate::api::ApiRegistry`].
pub fn factory() -> Arc<dyn ApiImpl> {
    Arc::new(GoogleVertexApi::new())
}

#[async_trait::async_trait]
impl ApiImpl for GoogleVertexApi {
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
        let auth_ctx = crate::auth::types::EnvAuthContext;

        macro_rules! fail {
            ($err:expr) => {{
                let e: ProviderError = $err;
                sink.send(e.into_error_event(provider, &model_id, Some(model.api.clone())))
                    .await;
                return;
            }};
        }

        // pi `stream` (`google-vertex.ts:98-102`): the api-key arm first, ADC otherwise.
        let api_key = resolve_api_key(auth.auth.api_key.as_deref());
        let (url, bearer) = match api_key {
            Some(_) => {
                // `createClientWithApiKey` (`:349-360`) — express mode: no project/location path.
                match express_url(model, auth) {
                    Ok(url) => (url, None),
                    Err(e) => fail!(e),
                }
            }
            None => {
                // `createClient` (`:327-339`) — project + location, bearer from ADC.
                let env = auth.env.as_ref();
                let project = match resolve_project(env) {
                    Ok(p) => p,
                    Err(e) => fail!(e),
                };
                let location = match resolve_location(env) {
                    Ok(l) => l,
                    Err(e) => fail!(e),
                };
                let url = match regional_url(model, auth, &project, &location) {
                    Ok(url) => url,
                    Err(e) => fail!(e),
                };
                match crate::auth::google_adc::resolve_access_token(&auth_ctx, env).await {
                    Ok(token) => (url, Some(token)),
                    Err(e) => fail!(e),
                }
            }
        };

        // The body is byte-for-byte the Gemini body — see the module note. PROV-011: an
        // unsatisfiable `constrainedSampling` fails the turn before any HTTP, and pi resolves the
        // Vertex leg through the same `resolveGoogleFunctionCallingMode` (`google-vertex.ts:469`
        // @v0.83.0) as the Gemini leg (`google-generative-ai.ts:370`).
        let params = match build_params(model, ctx, opts) {
            Ok(p) => p,
            Err(e) => fail!(ProviderError::from(e)),
        };
        let body = crate::stream::apply_on_payload(opts, model, params).await;
        let headers = build_headers(model, opts, api_key, bearer.as_deref());
        let req = SseRequest {
            method: reqwest::Method::POST,
            url,
            headers,
            body: Some(body),
        };

        let client = match build_client_for_target(
            &req.url,
            &auth_ctx,
            auth.env.as_ref(),
            opts.timeout_ms,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => fail!(e),
        };

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
            Err(e) => fail!(e),
        };
        capture.fire(opts, model).await;

        decode_stream(frames, model, &self.api, &sink).await;
    }
}

// ---------------------------------------------------------------------------
// Auth / option resolution
// ---------------------------------------------------------------------------

/// 1:1 port of pi `resolveApiKey` (`google-vertex.ts:388-394`): trim, then discard the
/// `gcp-vertex-credentials` marker and any `<placeholder>`. Returns `None` when there is no usable
/// key, which is what selects the ADC arm.
pub fn resolve_api_key(api_key: Option<&str>) -> Option<&str> {
    let key = api_key?.trim();
    if key.is_empty() || key == GCP_VERTEX_CREDENTIALS_MARKER || is_placeholder_api_key(key) {
        return None;
    }
    Some(key)
}

/// pi `isPlaceholderApiKey` (`google-vertex.ts:396-398`): `/^<[^>]+>$/`. Hand-rolled rather than
/// regex-driven because the pattern is anchored, three-token and total.
fn is_placeholder_api_key(api_key: &str) -> bool {
    let Some(inner) = api_key
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
    else {
        return false;
    };
    !inner.is_empty() && !inner.contains('>')
}

/// 1:1 port of pi `resolveProject` (`google-vertex.ts:396-406`), including the `GCLOUD_PROJECT`
/// fallback and the verbatim throw.
pub fn resolve_project(env: Option<&ProviderEnv>) -> Result<String, ProviderError> {
    provider_env_value(
        crate::providers::google_vertex::GOOGLE_CLOUD_PROJECT_ENV,
        env,
    )
    .or_else(|| provider_env_value(crate::providers::google_vertex::GCLOUD_PROJECT_ENV, env))
    .ok_or_else(|| ProviderError::Transport(MISSING_PROJECT_MESSAGE.into()))
}

/// 1:1 port of pi `resolveLocation` (`google-vertex.ts:408-414`).
pub fn resolve_location(env: Option<&ProviderEnv>) -> Result<String, ProviderError> {
    provider_env_value(
        crate::providers::google_vertex::GOOGLE_CLOUD_LOCATION_ENV,
        env,
    )
    .ok_or_else(|| ProviderError::Transport(MISSING_LOCATION_MESSAGE.into()))
}

// ---------------------------------------------------------------------------
// Endpoint construction
// ---------------------------------------------------------------------------

/// pi `resolveCustomBaseUrl` (`google-vertex.ts:362-368`): a blank base URL, or one still carrying
/// the `{location}` template placeholder, is NOT a custom override — the host is derived from the
/// location instead.
pub fn resolve_custom_base_url(base_url: &str) -> Option<&str> {
    let trimmed = base_url.trim();
    if trimmed.is_empty() || trimmed.contains(LOCATION_PLACEHOLDER) {
        return None;
    }
    Some(trimmed)
}

/// pi `baseUrlIncludesApiVersion` (`google-vertex.ts:370-377`): does any path segment look like
/// `v1`, `v1beta`, `v2beta3`…? When it does, pi clears `httpOptions.apiVersion` so the SDK does not
/// append a second one — reproduced here by not prefixing `/v1`.
pub fn base_url_includes_api_version(base_url: &str) -> bool {
    // pi's `try { new URL(...) }` arm inspects only the pathname; the `catch` arm falls back to a
    // scan of the whole string. A `str`-level scan of the segments after the authority reproduces
    // both without pulling in a URL parser.
    let after_scheme = base_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base_url);
    let path = match after_scheme.find('/') {
        Some(idx) => &after_scheme[idx..],
        // No path at all — pi's URL arm sees pathname `"/"`, which has no version segment.
        None if base_url.contains("://") => return false,
        None => after_scheme,
    };
    path.split('/').any(is_api_version_segment)
}

/// `/^v\d+(?:beta\d*)?$/`.
fn is_api_version_segment(segment: &str) -> bool {
    let Some(rest) = segment.strip_prefix('v') else {
        return false;
    };
    let digits_end = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits_end == 0 {
        return false;
    }
    match &rest[digits_end..] {
        "" => true,
        tail => tail
            .strip_prefix("beta")
            .is_some_and(|d| d.chars().all(|c| c.is_ascii_digit())),
    }
}

/// The `{location}`-interpolated regional host, per PROV-030's Fix ("The base-URL template
/// interpolation (`https://{location}-aiplatform.googleapis.com`) belongs in the new impl").
/// `location == "global"` selects the location-less host, which is what the Vertex `global`
/// endpoint requires.
pub fn interpolate_host(location: &str) -> String {
    if location == GLOBAL_LOCATION {
        return GLOBAL_HOST.to_string();
    }
    REGIONAL_HOST_TEMPLATE.replace(LOCATION_PLACEHOLDER, location)
}

/// The base URL an auth override / catalog row asks for, or `None` to derive one.
fn configured_base_url<'a>(model: &'a Model, auth: &'a AuthResult) -> Option<&'a str> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
    resolve_custom_base_url(base)
}

/// Join a host with the version prefix pi's `httpOptions.apiVersion` would contribute, then the
/// resource path.
fn join(host: &str, resource: &str) -> String {
    let host = host.trim_end_matches('/');
    if base_url_includes_api_version(host) {
        // pi sets `httpOptions.apiVersion = ""` in this case (`google-vertex.ts:355-357`).
        format!("{host}/{resource}")
    } else {
        format!("{host}/{API_VERSION}/{resource}")
    }
}

/// The express-mode (api-key) endpoint: `createClientWithApiKey` passes no project/location, so the
/// SDK addresses the publisher model directly on the global host.
pub fn express_url(model: &Model, auth: &AuthResult) -> Result<String, ProviderError> {
    let host = configured_base_url(model, auth).unwrap_or(GLOBAL_HOST);
    Ok(join(
        host,
        &format!(
            "publishers/google/models/{}:streamGenerateContent?alt=sse",
            model.id.as_str()
        ),
    ))
}

/// The ADC endpoint: the fully-qualified Vertex resource path under the regional (or global) host.
pub fn regional_url(
    model: &Model,
    auth: &AuthResult,
    project: &str,
    location: &str,
) -> Result<String, ProviderError> {
    let interpolated;
    let host = match configured_base_url(model, auth) {
        Some(custom) => custom,
        None => {
            interpolated = interpolate_host(location);
            interpolated.as_str()
        }
    };
    Ok(join(
        host,
        &format!(
            "projects/{project}/locations/{location}/publishers/google/models/{}:streamGenerateContent?alt=sse",
            model.id.as_str()
        ),
    ))
}

/// Build the request headers. Express mode authenticates with `x-goog-api-key` exactly as the
/// Gemini adapter does; the ADC arm sends the minted OAuth bearer. The model/opts header overlays
/// layer last, and a `None` value suppresses a default — pi `providerHeadersToRecord` applied via
/// `buildHttpOptions` (`google-vertex.ts:341-347`).
pub fn build_headers(
    model: &Model,
    opts: &StreamOptions,
    api_key: Option<&str>,
    bearer: Option<&str>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type".to_string(),
        Some("application/json".to_string()),
    );
    if let Some(key) = api_key {
        headers.insert("x-goog-api-key".to_string(), Some(key.to_string()));
    }
    if let Some(token) = bearer {
        headers.insert("authorization".to_string(), Some(format!("Bearer {token}")));
    }

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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::auth::types::ModelAuth;
    use crate::model::{Modality, ModelCost};
    use cyrup_core::{Content, Message};

    fn vertex_model(id: &str) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            api: API_ID.into(),
            provider: "google-vertex".into(),
            base_url: crate::providers::google_vertex::GOOGLE_VERTEX_BASE_URL_TEMPLATE.to_string(),
            reasoning: true,
            input: vec![Modality::Text, Modality::Image],
            cost: ModelCost {
                input: 1.25,
                output: 10.0,
                cache_read: 0.31,
                cache_write: 0.0,
                tiers: None,
            },
            context_window: 1_048_576,
            max_tokens: 65_536,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn keyless() -> AuthResult {
        AuthResult {
            auth: ModelAuth::default(),
            env: None,
            source: None,
        }
    }

    fn env_of(pairs: &[(&str, &str)]) -> ProviderEnv {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    // ------------------------------------------------------------------ resolveApiKey

    #[test]
    fn the_credentials_marker_is_not_an_api_key() {
        // pi `resolveApiKey`, google-vertex.ts:389-392: the marker selects ADC, and treating it as
        // a key would send the literal string `gcp-vertex-credentials` as `x-goog-api-key`.
        assert_eq!(resolve_api_key(Some(GCP_VERTEX_CREDENTIALS_MARKER)), None);
        assert_eq!(resolve_api_key(Some("  ")), None);
        assert_eq!(resolve_api_key(None), None);
        assert_eq!(resolve_api_key(Some("  AIza-real  ")), Some("AIza-real"));
    }

    #[test]
    fn angle_bracket_placeholders_are_not_api_keys() {
        assert_eq!(resolve_api_key(Some("<YOUR_API_KEY>")), None);
        assert_eq!(resolve_api_key(Some("<>")), Some("<>"), "`[^>]+` needs one char");
        assert_eq!(
            resolve_api_key(Some("<a>b>")),
            Some("<a>b>"),
            "`[^>]+>$` is anchored: an inner `>` breaks the match"
        );
    }

    // ------------------------------------------------------------------ project / location

    #[test]
    fn project_falls_back_to_gcloud_project_then_throws_pis_message() {
        let env = env_of(&[("GCLOUD_PROJECT", "legacy-proj")]);
        assert_eq!(resolve_project(Some(&env)).unwrap(), "legacy-proj");

        let preferred = env_of(&[
            ("GCLOUD_PROJECT", "legacy-proj"),
            ("GOOGLE_CLOUD_PROJECT", "new-proj"),
        ]);
        assert_eq!(resolve_project(Some(&preferred)).unwrap(), "new-proj");
    }

    #[test]
    fn a_missing_location_reports_pis_exact_sentence() {
        let err = resolve_location(Some(&env_of(&[]))).unwrap_err();
        assert!(
            err.to_string().contains(MISSING_LOCATION_MESSAGE),
            "got: {err}"
        );
    }

    // ------------------------------------------------------------------ endpoint

    #[test]
    fn the_adc_endpoint_interpolates_location_and_carries_the_resource_path() {
        // The single regression that PROV-030 is about: this URL did not exist at all, and every
        // Vertex request died in `wire.rs` with `no API implementation for google-vertex`.
        let model = vertex_model("gemini-2.5-pro");
        let url = regional_url(&model, &keyless(), "my-proj", "us-central1").unwrap();
        assert_eq!(
            url,
            "https://us-central1-aiplatform.googleapis.com/v1/projects/my-proj/locations/us-central1/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn the_global_location_uses_the_location_less_host() {
        let model = vertex_model("gemini-3.5-flash");
        let url = regional_url(&model, &keyless(), "p", GLOBAL_LOCATION).unwrap();
        assert!(
            url.starts_with("https://aiplatform.googleapis.com/v1/projects/p/locations/global/"),
            "got: {url}"
        );
    }

    #[test]
    fn express_mode_drops_the_project_and_location_segments() {
        let model = vertex_model("gemini-2.5-flash");
        let url = express_url(&model, &keyless()).unwrap();
        assert_eq!(
            url,
            "https://aiplatform.googleapis.com/v1/publishers/google/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn the_catalog_template_is_never_sent_verbatim() {
        // Every row of catalog/google-vertex.json carries the literal
        // `https://{location}-aiplatform.googleapis.com`. pi's `resolveCustomBaseUrl` exists
        // precisely so that string is never used as a host.
        for model in crate::providers::google_vertex::google_vertex_models() {
            assert!(
                resolve_custom_base_url(&model.base_url).is_none(),
                "{} leaked the template as a custom base url",
                model.id.as_str()
            );
            let url = regional_url(&model, &keyless(), "p", "europe-west4").unwrap();
            assert!(
                !url.contains(LOCATION_PLACEHOLDER),
                "{} kept the placeholder: {url}",
                model.id.as_str()
            );
        }
    }

    #[test]
    fn a_custom_base_url_overrides_the_template_and_is_version_aware() {
        let mut model = vertex_model("gemini-2.5-pro");
        model.base_url = "https://proxy.internal/vertex".to_string();
        let url = regional_url(&model, &keyless(), "p", "us-east1").unwrap();
        assert!(url.starts_with("https://proxy.internal/vertex/v1/projects/p/"), "got: {url}");

        // pi clears `apiVersion` when the base url already names one (`:355-357`), so `/v1` must
        // NOT be appended twice.
        model.base_url = "https://proxy.internal/v1beta1".to_string();
        let url = regional_url(&model, &keyless(), "p", "us-east1").unwrap();
        assert_eq!(
            url,
            "https://proxy.internal/v1beta1/projects/p/locations/us-east1/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse"
        );
    }

    #[test]
    fn api_version_detection_matches_pis_regex() {
        assert!(base_url_includes_api_version("https://h/v1"));
        assert!(base_url_includes_api_version("https://h/v1beta"));
        assert!(base_url_includes_api_version("https://h/v2beta3/x"));
        assert!(!base_url_includes_api_version("https://h"));
        assert!(!base_url_includes_api_version("https://h/vertex"));
        assert!(!base_url_includes_api_version("https://h/v1alpha"));
        assert!(!base_url_includes_api_version("https://v1.example.com/x"));
    }

    #[test]
    fn an_auth_base_url_override_beats_the_model_row() {
        let model = vertex_model("gemini-2.5-pro");
        let auth = AuthResult {
            auth: ModelAuth {
                base_url: Some("https://override.example/v1".to_string()),
                ..Default::default()
            },
            env: None,
            source: None,
        };
        assert!(
            regional_url(&model, &auth, "p", "us-east1")
                .unwrap()
                .starts_with("https://override.example/v1/projects/p/")
        );
    }

    // ------------------------------------------------------------------ headers

    #[test]
    fn the_adc_arm_sends_a_bearer_and_no_api_key_header() {
        let model = vertex_model("gemini-2.5-pro");
        let headers = build_headers(&model, &StreamOptions::default(), None, Some("ya29.tok"));
        assert_eq!(
            headers.get("authorization").cloned().flatten().as_deref(),
            Some("Bearer ya29.tok")
        );
        assert!(!headers.contains_key("x-goog-api-key"));
    }

    #[test]
    fn express_mode_sends_the_api_key_header_and_no_bearer() {
        let model = vertex_model("gemini-2.5-pro");
        let headers = build_headers(&model, &StreamOptions::default(), Some("AIza"), None);
        assert_eq!(
            headers.get("x-goog-api-key").cloned().flatten().as_deref(),
            Some("AIza")
        );
        assert!(!headers.contains_key("authorization"));
    }

    #[test]
    fn opts_headers_layer_over_model_headers_and_can_suppress_a_default() {
        let mut model = vertex_model("gemini-2.5-pro");
        let mut model_headers = HeaderMap::new();
        model_headers.insert("x-tenant".to_string(), Some("a".to_string()));
        model.headers = Some(model_headers);

        let mut opts = StreamOptions::default();
        let mut opt_headers = HeaderMap::new();
        opt_headers.insert("x-tenant".to_string(), Some("b".to_string()));
        opt_headers.insert("content-type".to_string(), None);
        opts.headers = Some(opt_headers);

        let headers = build_headers(&model, &opts, None, Some("tok"));
        assert_eq!(headers.get("x-tenant").cloned().flatten().as_deref(), Some("b"));
        assert_eq!(headers.get("content-type"), Some(&None));
    }

    // ------------------------------------------------------------------ registration

    #[test]
    fn the_registry_now_serves_google_vertex() {
        // PROV-030's own Verify clause: every model every built-in provider ships must resolve to a
        // registered api. Before this module, all 10 Vertex rows failed it.
        let registry = crate::api::builtin_registry();
        assert!(registry.contains(&ApiId::from(API_ID)));
        for model in crate::providers::google_vertex::google_vertex_models() {
            assert!(
                registry.contains(&model.api),
                "{} still dangles on api {}",
                model.id.as_str(),
                model.api
            );
        }
    }

    #[test]
    fn the_body_is_the_gemini_body() {
        // The claim this module rests on: pi's two `buildParams` are identical, so delegating is a
        // port and not a shortcut. Assert the delegate really is producing a Gemini payload.
        let model = vertex_model("gemini-2.5-pro");
        let ctx = Context {
            system_prompt: Some("be brief".to_string()),
            messages: vec![Message::User {
                content: vec![Content::text("hi")],
                timestamp: 0,
            }],
            tools: Vec::new(),
        };
        let body = build_params(&model, &ctx, &StreamOptions::default()).unwrap();
        assert!(body.get("contents").is_some(), "got: {body}");
    }
}
