//! The Google Vertex AI provider (arch-01 §5) — a 1:1 port of pi v0.83.0
//! `packages/ai/src/providers/google-vertex.ts`.
//!
//! Vertex is the only built-in whose credential can be *nothing at all*: `gcloud auth
//! application-default login` writes a JSON file under `~/.config/gcloud/`, and the provider then
//! authenticates out of that file plus two env vars naming the GCP project and region
//! (`google-vertex.ts:8-11`). So [`GoogleVertexApiKeyAuth::resolve`] has two arms — an explicit API
//! key, or the ADC triple (credentials file present **and** project **and** location) which
//! resolves to an [`AuthResult`] carrying *no* key at all, only the env overlay
//! (`google-vertex.ts:62-87`).
//!
//! Every model's `baseUrl` is the template `https://{location}-aiplatform.googleapis.com`
//! (`google-vertex.models.ts`), interpolated by the wire impl from the resolved location — which is
//! why the ADC arm must return the env overlay even though it returns no key.
//!
//! # Catalog provenance — read before refreshing
//!
//! The 10 models in `catalog/google-vertex.json` are the verbatim contents of pi
//! `packages/ai/src/providers/google-vertex.models.ts` at commit `b0c2a90e` (2026-07-17), the LAST
//! revision at which pi tracks this catalog's literal data in git. One commit later (`a9f6a315`,
//! "feat(ai): separate generated model data") the data moved to
//! `packages/ai/src/providers/data/google-vertex.json`, which `.gitignore:11` excludes — so at the
//! ported tag `v0.83.0` the catalog is not obtainable from the repository at all, and `b0c2a90e` is
//! the closest knowable snapshot to it. This is the same revision
//! `providers/catalog/github-copilot.json` was extracted from; see that module's note for why a
//! revision newer than `providers/catalog_manifest.json`'s `91585d9a` cannot violate the manifest's
//! staleness *floor* invariant (`providers/all.rs:76-83`).
//!
//! # What is not here
//!
//! * **The wire api — PROV-030, still open.** Every row's `api` is `google-vertex`, and this crate
//!   has no `api/google_vertex.rs`: [`crate::api::register_builtins`] registers nine impls and that
//!   is not among them. The provider IS registered in [`crate::providers::all`], so all 10 rows
//!   resolve auth, list in `/model`, and then fail the registry lookup with a terminal
//!   `StreamEvent::Error` (`wire.rs`, R-01-008/017/018) — the catalog and the auth precedence below
//!   are complete and tested, the transport is not. Upstream's is
//!   `packages/ai/src/api/google-vertex.ts` (591 lines at `v0.83.0`), which drives the
//!   `@google/genai` SDK in Vertex mode; porting it additionally requires minting a bearer from the
//!   ADC file (an `authorized_user` refresh-token exchange, and RS256 JWT signing for the
//!   `service-account` arm this module's `login` can now store), which this crate has no dependency
//!   for today.
//! * **`vertexAuth.login`** (`google-vertex.ts:15-61`) is ported below — the three-way interactive
//!   picker (API key / ADC / service-account file) that *writes* the credential — on
//!   [`crate::auth::ApiKeyAuth::login`], the trait member CFG-005 added beside `name` + `resolve`
//!   (pi `ai/src/auth/types.ts:166`).

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::types::{AuthContext, AuthResult, Credential, ModelAuth, ProviderEnv};
use crate::auth::{ApiKeyAuth, CredentialStore, InMemoryCredentialStore, ProviderAuth};
use crate::error::AuthError;
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// The provider id (pi `google-vertex.ts:90`).
pub const GOOGLE_VERTEX_PROVIDER_ID: &str = "google-vertex";

/// The wire-protocol id every catalog row declares (pi `Provider<"google-vertex">`,
/// `google-vertex.ts:89`). Not in [`crate::known_api`] because no impl is registered for it yet;
/// see the module note.
pub const GOOGLE_VERTEX_API: &str = "google-vertex";

/// The per-model base-URL template (pi `google-vertex.models.ts`). `{location}` is the GCP region.
pub const GOOGLE_VERTEX_BASE_URL_TEMPLATE: &str = "https://{location}-aiplatform.googleapis.com";

/// Where `gcloud auth application-default login` writes its credentials (pi `VERTEX_ADC_PATH`,
/// `google-vertex.ts:6`). Used verbatim, leading `~` and all — pi hands the same literal to
/// `ctx.fileExists`, which expands it (`auth/context.ts:31-36`).
pub const VERTEX_ADC_PATH: &str = "~/.config/gcloud/application_default_credentials.json";

/// An explicit Google Cloud API key (pi `google-vertex.ts:63`).
pub const GOOGLE_CLOUD_API_KEY_ENV: &str = "GOOGLE_CLOUD_API_KEY";

/// Path override for the ADC credentials file (pi `google-vertex.ts:67-68`).
pub const GOOGLE_APPLICATION_CREDENTIALS_ENV: &str = "GOOGLE_APPLICATION_CREDENTIALS";

/// The GCP project id (pi `google-vertex.ts:71-73`).
pub const GOOGLE_CLOUD_PROJECT_ENV: &str = "GOOGLE_CLOUD_PROJECT";

/// Legacy alias for the project id, consulted only after [`GOOGLE_CLOUD_PROJECT_ENV`] (pi
/// `google-vertex.ts:73`).
pub const GCLOUD_PROJECT_ENV: &str = "GCLOUD_PROJECT";

/// The GCP region (pi `google-vertex.ts:74`).
pub const GOOGLE_CLOUD_LOCATION_ENV: &str = "GOOGLE_CLOUD_LOCATION";

/// The three `vertexAuth.login` option ids (pi `google-vertex.ts:20-22`). They are the values the
/// select prompt returns, so they are the flow's contract with the UI.
const METHOD_API_KEY: &str = "api-key";
const METHOD_ADC: &str = "adc";
const METHOD_SERVICE_ACCOUNT: &str = "service-account";

/// The `vertexAuth.login` prompt/notify strings, verbatim from `google-vertex.ts:18`, `:28`,
/// `:38-43` and `:47-51`.
const SELECT_AUTH_METHOD_MESSAGE: &str = "Select Google Vertex AI authentication method:";
const ENTER_GOOGLE_CLOUD_API_KEY: &str = "Enter Google Cloud API key";
const ADC_INFO_MESSAGE: &str =
    "Run `gcloud auth application-default login`, then provide the project and location.";
const SERVICE_ACCOUNT_INFO_MESSAGE: &str =
    "Provide a service account credentials file, project, and location.";
const ADC_DOCS_URL: &str = "https://cloud.google.com/docs/authentication/provide-credentials-adc";
const ENTER_PROJECT_MESSAGE: &str = "Enter Google Cloud project ID";
const ENTER_LOCATION_MESSAGE: &str = "Enter Google Cloud location";
const ENTER_CREDENTIALS_PATH_MESSAGE: &str = "Enter service account credentials file path";

/// `source` when the value came off the stored credential (pi `google-vertex.ts:65`/`:81`).
const SOURCE_STORED_CREDENTIAL: &str = "stored credential";

/// `source` for the ADC arm (pi `google-vertex.ts:81`).
const SOURCE_ADC: &str = "gcloud application default credentials";

/// The verbatim catalog extracted from pi's generated `google-vertex.models.ts` (see the
/// module-level provenance note).
const GOOGLE_VERTEX_CATALOG_JSON: &str = include_str!("catalog/google-vertex.json");

/// The full Vertex catalog (1:1 with pi `GOOGLE_VERTEX_MODELS`). A parse failure yields an empty
/// catalog (surfaced loudly by the count test) rather than a panic (NO-PANIC policy).
pub fn google_vertex_models() -> Vec<Model> {
    serde_json::from_str(GOOGLE_VERTEX_CATALOG_JSON).unwrap_or_default()
}

/// The Vertex [`ProviderAuth`] (pi `auth: { apiKey: vertexAuth }`, `google-vertex.ts:93`).
pub fn google_vertex_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(Arc::new(GoogleVertexApiKeyAuth))
}

/// Construct the Vertex provider over the given credential store + shared api registry.
///
/// The registry must provide the `google-vertex` impl; none is registered today (see the module
/// note), so this provider resolves models and auth but cannot yet stream.
pub fn google_vertex_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        GOOGLE_VERTEX_PROVIDER_ID,
        "Google Vertex AI",
        google_vertex_models(),
        google_vertex_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn google_vertex_provider() -> WireProvider {
    google_vertex_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
}

/// pi `vertexAuth` (`google-vertex.ts:16-88`) — the `resolve` half.
///
/// Vertex is not an `envApiKeyAuth`: it hand-rolls resolution because the ADC arm has no key to
/// return. The precedence is exactly upstream's, including two JS operators whose semantics differ
/// from the obvious Rust reading and are reproduced deliberately:
///
/// * `??` (nullish coalescing) — `credential?.key ?? await ctx.env(...)` (`:63`). A stored key of
///   `""` is **not** nullish, so it wins the coalesce and *suppresses* the env lookup; the
///   subsequent `if (key)` then fails and resolution falls through to the ADC arm. A stored empty
///   key therefore means "no API key", never "read the env".
/// * `if (value)` (truthiness) — `""` is falsy, so an empty key/project/location is absent
///   (`:64`, `:76`).
pub struct GoogleVertexApiKeyAuth;

/// `credential?.key` — `undefined` for a missing credential, and for an OAuth one (pi types the
/// argument as `ApiKeyCredential`, so an OAuth credential simply has no `key` member).
fn credential_key(cred: Option<&Credential>) -> Option<&String> {
    match cred {
        Some(Credential::ApiKey { key, .. }) => key.as_ref(),
        _ => None,
    }
}

/// `credential?.env?.<name>` (`:67`, `:72`, `:74`).
fn credential_env_var<'a>(env: Option<&'a ProviderEnv>, name: &str) -> Option<&'a String> {
    env.and_then(|e| e.get(name))
}

#[async_trait::async_trait]
impl ApiKeyAuth for GoogleVertexApiKeyAuth {
    /// pi `vertexAuth.name` (`google-vertex.ts:17`).
    fn name(&self) -> &str {
        "Google Cloud credentials"
    }

    fn supports_login(&self) -> bool {
        true
    }

    /// 1:1 port of `vertexAuth.login` (`google-vertex.ts:15-61`): a three-way picker, then either
    /// one secret prompt (API key) or the project/location pair — plus the credentials-file path on
    /// the `service-account` arm. An unknown option id is upstream's
    /// `Unknown Google Vertex AI auth method: {method}` (`:32`).
    ///
    /// CFG-005: two of the three arms store NO key at all, only an env overlay — a `/login` that
    /// assumes a single secret cannot express them.
    async fn login(
        &self,
        interaction: &dyn crate::auth::oauth::AuthInteraction,
    ) -> Result<Credential, crate::auth::oauth::OAuthError> {
        use crate::auth::oauth::{
            AuthEvent, AuthInfoLink, AuthPrompt, AuthSelectOption, OAuthError,
        };

        // `:16-24`
        let method = interaction
            .prompt(AuthPrompt::select(
                SELECT_AUTH_METHOD_MESSAGE,
                vec![
                    AuthSelectOption {
                        id: METHOD_API_KEY.to_string(),
                        label: "Google Cloud API key".to_string(),
                        description: None,
                    },
                    AuthSelectOption {
                        id: METHOD_ADC.to_string(),
                        label: "Application Default Credentials".to_string(),
                        description: None,
                    },
                    AuthSelectOption {
                        id: METHOD_SERVICE_ACCOUNT.to_string(),
                        label: "Service account credentials file".to_string(),
                        description: None,
                    },
                ],
            ))
            .await?;

        // `:25-30`
        if method == METHOD_API_KEY {
            let key = interaction
                .prompt(AuthPrompt::secret(ENTER_GOOGLE_CLOUD_API_KEY))
                .await?;
            return Ok(Credential::ApiKey {
                key: Some(key),
                env: None,
            });
        }
        // `:31-33`
        if method != METHOD_ADC && method != METHOD_SERVICE_ACCOUNT {
            return Err(OAuthError::Failed(format!(
                "Unknown Google Vertex AI auth method: {method}"
            )));
        }

        // `:34-46`
        interaction.notify(AuthEvent::Info {
            message: if method == METHOD_ADC {
                ADC_INFO_MESSAGE.to_string()
            } else {
                SERVICE_ACCOUNT_INFO_MESSAGE.to_string()
            },
            links: vec![AuthInfoLink {
                label: Some("Application Default Credentials".to_string()),
                url: ADC_DOCS_URL.to_string(),
            }],
        });

        // `:47-52`
        let project = interaction
            .prompt(AuthPrompt::text(ENTER_PROJECT_MESSAGE))
            .await?;
        let location = interaction
            .prompt(AuthPrompt::text(ENTER_LOCATION_MESSAGE))
            .await?;
        let credentials_path = if method == METHOD_SERVICE_ACCOUNT {
            Some(
                interaction
                    .prompt(AuthPrompt::text(ENTER_CREDENTIALS_PATH_MESSAGE))
                    .await?,
            )
        } else {
            None
        };

        // `:53-60` — no `key` on either arm; the env overlay is the whole credential.
        let mut env = ProviderEnv::new();
        env.insert(GOOGLE_CLOUD_PROJECT_ENV.to_string(), project);
        env.insert(GOOGLE_CLOUD_LOCATION_ENV.to_string(), location);
        if let Some(path) = credentials_path {
            env.insert(GOOGLE_APPLICATION_CREDENTIALS_ENV.to_string(), path);
        }
        Ok(Credential::ApiKey {
            key: None,
            env: Some(env),
        })
    }

    async fn resolve(
        &self,
        _model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let cred_key = credential_key(cred);
        let cred_env = cred.and_then(Credential::env);

        // `const key = credential?.key ?? (await ctx.env("GOOGLE_CLOUD_API_KEY"));` (`:63`).
        let key = match cred_key {
            Some(key) => Some(key.clone()),
            None => ctx.env(GOOGLE_CLOUD_API_KEY_ENV).await,
        };
        // `if (key) return { auth: { apiKey: key }, source: credential?.key ? … : … };` (`:64`).
        if let Some(key) = key.filter(|k| !k.is_empty()) {
            let source = if cred_key.is_some() {
                SOURCE_STORED_CREDENTIAL
            } else {
                GOOGLE_CLOUD_API_KEY_ENV
            };
            return Ok(Some(AuthResult {
                auth: ModelAuth {
                    api_key: Some(key),
                    ..Default::default()
                },
                // pi returns no `env` on this arm (`:64`).
                env: None,
                source: Some(source.to_string()),
            }));
        }

        // `credential?.env?.GOOGLE_APPLICATION_CREDENTIALS ?? (await ctx.env(…))` (`:67-68`).
        let adc_path = match credential_env_var(cred_env, GOOGLE_APPLICATION_CREDENTIALS_ENV) {
            Some(path) => Some(path.clone()),
            None => ctx.env(GOOGLE_APPLICATION_CREDENTIALS_ENV).await,
        };
        // `await ctx.fileExists(adcPath ?? VERTEX_ADC_PATH)` (`:69`).
        let has_credentials = ctx
            .file_exists(adc_path.as_deref().unwrap_or(VERTEX_ADC_PATH))
            .await;

        // `credential?.env?.GOOGLE_CLOUD_PROJECT ?? ctx.env("GOOGLE_CLOUD_PROJECT")
        //  ?? ctx.env("GCLOUD_PROJECT")` (`:71-74`).
        let project = match credential_env_var(cred_env, GOOGLE_CLOUD_PROJECT_ENV) {
            Some(project) => Some(project.clone()),
            None => match ctx.env(GOOGLE_CLOUD_PROJECT_ENV).await {
                Some(project) => Some(project),
                None => ctx.env(GCLOUD_PROJECT_ENV).await,
            },
        };
        // `credential?.env?.GOOGLE_CLOUD_LOCATION ?? ctx.env("GOOGLE_CLOUD_LOCATION")` (`:75`).
        let location = match credential_env_var(cred_env, GOOGLE_CLOUD_LOCATION_ENV) {
            Some(location) => Some(location.clone()),
            None => ctx.env(GOOGLE_CLOUD_LOCATION_ENV).await,
        };

        // `if (hasCredentials && project && location)` (`:76`) — JS truthiness drops `""`.
        let configured = has_credentials
            && project.is_some_and(|p| !p.is_empty())
            && location.is_some_and(|l| !l.is_empty());
        if configured {
            return Ok(Some(AuthResult {
                // `auth: {}` — the Vertex wire impl mints its own bearer from the ADC file
                // (`:78`).
                auth: ModelAuth::default(),
                // `env: credential?.env` (`:79`).
                env: cred_env.cloned(),
                // `source: credential ? "stored credential" : "gcloud application default
                // credentials"` — keyed on the CREDENTIAL, not on `credential.key` (`:80`).
                source: Some(
                    if cred.is_some() {
                        SOURCE_STORED_CREDENTIAL
                    } else {
                        SOURCE_ADC
                    }
                    .to_string(),
                ),
            }));
        }

        // `return undefined` — not configured (`:83`).
        Ok(None)
    }
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
    use crate::model::Modality;
    use crate::provider::Provider;
    use std::collections::{BTreeMap, BTreeSet};

    // ------------------------------------------------------------------ fixtures

    /// An [`AuthContext`] over a fixed env map + a fixed set of existing paths. Nothing here
    /// touches the real environment or the real filesystem.
    struct FakeCtx {
        env: BTreeMap<String, String>,
        files: BTreeSet<String>,
    }

    impl FakeCtx {
        fn new() -> Self {
            FakeCtx {
                env: BTreeMap::new(),
                files: BTreeSet::new(),
            }
        }
        fn with_env(mut self, name: &str, value: &str) -> Self {
            self.env.insert(name.to_string(), value.to_string());
            self
        }
        fn with_file(mut self, path: &str) -> Self {
            self.files.insert(path.to_string());
            self
        }
    }

    #[async_trait::async_trait]
    impl AuthContext for FakeCtx {
        async fn env(&self, name: &str) -> Option<String> {
            self.env.get(name).cloned()
        }
        async fn file_exists(&self, path: &str) -> bool {
            self.files.contains(path)
        }
    }

    fn any_model() -> Model {
        google_vertex_models()
            .into_iter()
            .next()
            .expect("catalog is non-empty")
    }

    async fn resolve(ctx: &FakeCtx, cred: Option<&Credential>) -> Option<AuthResult> {
        GoogleVertexApiKeyAuth
            .resolve(&any_model(), ctx, cred)
            .await
            .expect("vertex resolve never errors")
    }

    fn env_credential(pairs: &[(&str, &str)]) -> Credential {
        let mut env = ProviderEnv::new();
        for (k, v) in pairs {
            env.insert((*k).to_string(), (*v).to_string());
        }
        Credential::ApiKey {
            key: None,
            env: Some(env),
        }
    }

    // ------------------------------------------------------------------ catalog

    /// pi `GOOGLE_VERTEX_MODELS` at `b0c2a90e`: 10 models, all on the `google-vertex` wire api and
    /// all on the `{location}` base-URL template.
    #[test]
    fn catalog_parses_verbatim_with_expected_count() {
        let models = google_vertex_models();
        assert_eq!(models.len(), 10);
        assert!(
            models
                .iter()
                .all(|m| m.provider.as_str() == GOOGLE_VERTEX_PROVIDER_ID)
        );
        assert!(models.iter().all(|m| m.api.as_str() == GOOGLE_VERTEX_API));
        assert!(
            models
                .iter()
                .all(|m| m.base_url == GOOGLE_VERTEX_BASE_URL_TEMPLATE)
        );
        // Every Vertex row is a reasoning, text+image model with a 1 048 576-token window.
        assert!(models.iter().all(|m| m.reasoning));
        assert!(models.iter().all(|m| m.supports_image_input()));
        assert!(models.iter().all(|m| m.context_window == 1_048_576));
        assert!(models.iter().all(|m| m.max_tokens == 65_536));

        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "gemini-2.5-flash",
                "gemini-2.5-flash-lite",
                "gemini-2.5-pro",
                "gemini-3-flash-preview",
                "gemini-3.1-flash-lite",
                "gemini-3.1-pro-preview",
                "gemini-3.1-pro-preview-customtools",
                "gemini-3.5-flash",
                "gemini-flash-latest",
                "gemini-flash-lite-latest",
            ]
        );
    }

    /// The base URL is a TEMPLATE, not a host: `{location}` must survive into the catalog for the
    /// wire impl to interpolate. A catalog that "helpfully" baked in a region would break every
    /// non-default deployment.
    #[test]
    fn base_url_keeps_the_location_placeholder() {
        for m in google_vertex_models() {
            assert!(
                m.base_url.contains("{location}"),
                "{} lost the location placeholder",
                m.id.as_str()
            );
        }
    }

    /// Spot-check the row with the richest `thinkingLevelMap`: `gemini-3.1-pro-preview` maps three
    /// levels to `null` (unsupported) and two to upper-case Vertex values (`google-vertex.models.ts`
    /// @`b0c2a90e`). MIRROR: `gemini-2.5-flash` carries NO map at all, so `None` here is upstream's
    /// omission and not a parse failure that swallowed every map.
    #[test]
    fn thinking_level_maps_match_the_upstream_rows() {
        let models = google_vertex_models();
        let by_id = |id: &str| {
            models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("{id} missing from catalog"))
        };

        let pro = by_id("gemini-3.1-pro-preview");
        let map = pro.thinking_level_map.as_ref().expect("thinkingLevelMap");
        assert_eq!(map.get("off"), Some(&None));
        assert_eq!(map.get("minimal"), Some(&None));
        assert_eq!(map.get("low"), Some(&Some("LOW".to_string())));
        assert_eq!(map.get("medium"), Some(&None));
        assert_eq!(map.get("high"), Some(&Some("HIGH".to_string())));
        assert_eq!(map.len(), 5);
        assert_eq!(pro.cost.input, 2.0);
        assert_eq!(pro.cost.output, 12.0);
        assert_eq!(pro.cost.cache_read, 0.2);
        assert_eq!(pro.cost.cache_write, 0.0);

        // `off: null` only.
        let flash3 = by_id("gemini-3-flash-preview");
        let map = flash3.thinking_level_map.as_ref().expect("thinkingLevelMap");
        assert_eq!(map.get("off"), Some(&None));
        assert_eq!(map.len(), 1);

        // MIRROR: the 2.5 rows have no map.
        assert!(by_id("gemini-2.5-flash").thinking_level_map.is_none());
        assert!(by_id("gemini-2.5-pro").thinking_level_map.is_none());
    }

    /// `gemini-2.5-pro` verbatim from `google-vertex.models.ts` @`b0c2a90e`.
    #[test]
    fn gemini_2_5_pro_matches_the_upstream_row() {
        let models = google_vertex_models();
        let m = models
            .iter()
            .find(|m| m.id.as_str() == "gemini-2.5-pro")
            .expect("gemini-2.5-pro");
        assert_eq!(m.name, "Gemini 2.5 Pro");
        assert_eq!(m.input, vec![Modality::Text, Modality::Image]);
        assert_eq!(m.cost.input, 1.25);
        assert_eq!(m.cost.output, 10.0);
        assert_eq!(m.cost.cache_read, 0.125);
        assert_eq!(m.cost.cache_write, 0.0);
        // Vertex rows carry no long-context tiers and no compat overrides.
        assert!(m.cost.tiers.is_none());
        assert!(m.compat.is_none());
        assert!(m.headers.is_none());
    }

    // ------------------------------------------------------------------ provider shape

    #[test]
    fn provider_exposes_the_upstream_id_and_name() {
        let provider = google_vertex_provider();
        assert_eq!(provider.id().as_str(), "google-vertex");
        assert_eq!(provider.name(), "Google Vertex AI");
        assert_eq!(provider.models().len(), 10);
        let auth = provider.provider_auth().expect("vertex declares auth");
        // pi wires `auth: { apiKey: vertexAuth }` — an api-key strategy, no OAuth
        // (`google-vertex.ts:93`).
        assert!(auth.api_key.is_some());
        assert!(auth.oauth.is_none());
        assert_eq!(
            auth.api_key.as_ref().map(|a| a.name().to_string()),
            Some("Google Cloud credentials".to_string())
        );
    }

    // ------------------------------------------------------------------ resolve: the API-key arm

    /// pi `:63-65`: the env key resolves when nothing is stored, and `source` names the variable.
    #[tokio::test]
    async fn env_api_key_resolves_with_the_variable_name_as_source() {
        let ctx = FakeCtx::new().with_env(GOOGLE_CLOUD_API_KEY_ENV, "AIzaEnvKey");
        let resolved = resolve(&ctx, None).await.expect("configured");
        assert_eq!(resolved.auth.api_key.as_deref(), Some("AIzaEnvKey"));
        assert_eq!(resolved.source.as_deref(), Some("GOOGLE_CLOUD_API_KEY"));
        assert!(resolved.env.is_none());
    }

    /// pi `:63-65`: a stored key outranks the env var and reports `"stored credential"`.
    #[tokio::test]
    async fn stored_api_key_outranks_the_env_key() {
        let ctx = FakeCtx::new().with_env(GOOGLE_CLOUD_API_KEY_ENV, "AIzaEnvKey");
        let cred = Credential::api_key("AIzaStoredKey");
        let resolved = resolve(&ctx, Some(&cred)).await.expect("configured");
        assert_eq!(resolved.auth.api_key.as_deref(), Some("AIzaStoredKey"));
        assert_eq!(resolved.source.as_deref(), Some("stored credential"));
    }

    /// The `??` in `credential?.key ?? await ctx.env(...)` (`:63`) is NULLISH, not falsy: a stored
    /// empty key wins the coalesce, suppresses the env lookup, and then fails `if (key)` — so
    /// resolution drops to the ADC arm and, with no ADC file, reports "not configured" even though
    /// `GOOGLE_CLOUD_API_KEY` is set.
    ///
    /// MIRROR: the identical context with the key REMOVED from the credential does read the env,
    /// so this pins the operator and not merely "empty keys are ignored".
    #[tokio::test]
    async fn a_stored_empty_key_suppresses_the_env_key() {
        let ctx = FakeCtx::new().with_env(GOOGLE_CLOUD_API_KEY_ENV, "AIzaEnvKey");

        let empty = Credential::ApiKey {
            key: Some(String::new()),
            env: None,
        };
        assert!(resolve(&ctx, Some(&empty)).await.is_none());

        let no_key = Credential::ApiKey {
            key: None,
            env: None,
        };
        let mirrored = resolve(&ctx, Some(&no_key)).await.expect("configured");
        assert_eq!(mirrored.auth.api_key.as_deref(), Some("AIzaEnvKey"));
    }

    // ------------------------------------------------------------------ resolve: the ADC arm

    /// pi `:67-82`: credentials file at the default `~` path + project + location resolves to a
    /// KEYLESS auth with the ADC source label.
    #[tokio::test]
    async fn application_default_credentials_resolve_without_a_key() {
        let ctx = FakeCtx::new()
            .with_file(VERTEX_ADC_PATH)
            .with_env(GOOGLE_CLOUD_PROJECT_ENV, "my-project")
            .with_env(GOOGLE_CLOUD_LOCATION_ENV, "us-central1");
        let resolved = resolve(&ctx, None).await.expect("configured");
        assert!(resolved.auth.api_key.is_none());
        assert!(resolved.auth.headers.is_none());
        assert!(resolved.auth.base_url.is_none());
        assert!(resolved.env.is_none());
        assert_eq!(
            resolved.source.as_deref(),
            Some("gcloud application default credentials")
        );
    }

    /// pi `:73`: `GCLOUD_PROJECT` is consulted only after `GOOGLE_CLOUD_PROJECT`, and both satisfy
    /// the ADC triple.
    #[tokio::test]
    async fn gcloud_project_is_the_project_fallback() {
        let ctx = FakeCtx::new()
            .with_file(VERTEX_ADC_PATH)
            .with_env(GCLOUD_PROJECT_ENV, "legacy-project")
            .with_env(GOOGLE_CLOUD_LOCATION_ENV, "europe-west4");
        assert!(resolve(&ctx, None).await.is_some());
    }

    /// pi `:67-69`: `GOOGLE_APPLICATION_CREDENTIALS` replaces the `~` default path, so the default
    /// path existing is neither necessary nor sufficient once the override is set.
    #[tokio::test]
    async fn google_application_credentials_overrides_the_default_adc_path() {
        let base = FakeCtx::new()
            .with_env(GOOGLE_CLOUD_PROJECT_ENV, "p")
            .with_env(GOOGLE_CLOUD_LOCATION_ENV, "l");

        // The override points at a file that exists → configured.
        let ok = FakeCtx {
            env: base.env.clone(),
            files: base.files.clone(),
        }
        .with_env(GOOGLE_APPLICATION_CREDENTIALS_ENV, "/srv/sa.json")
        .with_file("/srv/sa.json");
        assert!(resolve(&ok, None).await.is_some());

        // The override points elsewhere → the default path is NOT consulted, so this is not
        // configured even though the `~` file exists.
        let shadowed = FakeCtx {
            env: base.env.clone(),
            files: base.files.clone(),
        }
        .with_env(GOOGLE_APPLICATION_CREDENTIALS_ENV, "/srv/missing.json")
        .with_file(VERTEX_ADC_PATH);
        assert!(resolve(&shadowed, None).await.is_none());
    }

    /// pi `:76`: the ADC arm needs ALL THREE of file, project and location. Each one missing
    /// yields `undefined`; the complete triple (the MIRROR) resolves.
    #[tokio::test]
    async fn the_adc_triple_is_all_or_nothing() {
        let complete = FakeCtx::new()
            .with_file(VERTEX_ADC_PATH)
            .with_env(GOOGLE_CLOUD_PROJECT_ENV, "p")
            .with_env(GOOGLE_CLOUD_LOCATION_ENV, "l");
        assert!(resolve(&complete, None).await.is_some(), "MIRROR");

        // No credentials file.
        let no_file = FakeCtx::new()
            .with_env(GOOGLE_CLOUD_PROJECT_ENV, "p")
            .with_env(GOOGLE_CLOUD_LOCATION_ENV, "l");
        assert!(resolve(&no_file, None).await.is_none());

        // No project.
        let no_project = FakeCtx::new()
            .with_file(VERTEX_ADC_PATH)
            .with_env(GOOGLE_CLOUD_LOCATION_ENV, "l");
        assert!(resolve(&no_project, None).await.is_none());

        // No location.
        let no_location = FakeCtx::new()
            .with_file(VERTEX_ADC_PATH)
            .with_env(GOOGLE_CLOUD_PROJECT_ENV, "p");
        assert!(resolve(&no_location, None).await.is_none());

        // JS truthiness: an EMPTY project or location is absent (`:76`).
        let empty_project = FakeCtx::new()
            .with_file(VERTEX_ADC_PATH)
            .with_env(GOOGLE_CLOUD_PROJECT_ENV, "")
            .with_env(GOOGLE_CLOUD_LOCATION_ENV, "l");
        assert!(resolve(&empty_project, None).await.is_none());
        let empty_location = FakeCtx::new()
            .with_file(VERTEX_ADC_PATH)
            .with_env(GOOGLE_CLOUD_PROJECT_ENV, "p")
            .with_env(GOOGLE_CLOUD_LOCATION_ENV, "");
        assert!(resolve(&empty_location, None).await.is_none());
    }

    /// pi `:71-80`: a stored credential's `env` supplies the whole triple with no process env at
    /// all, is echoed back as the request-scoped overlay, and flips `source` to "stored
    /// credential".
    #[tokio::test]
    async fn a_stored_env_credential_supplies_the_whole_triple() {
        let ctx = FakeCtx::new().with_file("/keys/sa.json");
        let cred = env_credential(&[
            (GOOGLE_APPLICATION_CREDENTIALS_ENV, "/keys/sa.json"),
            (GOOGLE_CLOUD_PROJECT_ENV, "stored-project"),
            (GOOGLE_CLOUD_LOCATION_ENV, "asia-northeast1"),
        ]);
        let resolved = resolve(&ctx, Some(&cred)).await.expect("configured");
        assert!(resolved.auth.api_key.is_none());
        assert_eq!(resolved.source.as_deref(), Some("stored credential"));
        let env = resolved.env.expect("the credential env is echoed back");
        assert_eq!(
            env.get(GOOGLE_CLOUD_PROJECT_ENV).map(String::as_str),
            Some("stored-project")
        );
        assert_eq!(
            env.get(GOOGLE_CLOUD_LOCATION_ENV).map(String::as_str),
            Some("asia-northeast1")
        );
    }

    /// pi `:80` keys the ADC `source` on `credential`, NOT on `credential.key`: a credential that
    /// carries only an env overlay still reports "stored credential", even when the project and
    /// location come from the process env.
    #[tokio::test]
    async fn adc_source_is_keyed_on_the_credential_not_its_key() {
        let ctx = FakeCtx::new()
            .with_file(VERTEX_ADC_PATH)
            .with_env(GOOGLE_CLOUD_PROJECT_ENV, "p")
            .with_env(GOOGLE_CLOUD_LOCATION_ENV, "l");
        // An empty (but present) env overlay is still a credential.
        let cred = env_credential(&[]);
        let resolved = resolve(&ctx, Some(&cred)).await.expect("configured");
        assert_eq!(resolved.source.as_deref(), Some("stored credential"));

        // MIRROR: no credential at all → the ADC label.
        let resolved = resolve(&ctx, None).await.expect("configured");
        assert_eq!(
            resolved.source.as_deref(),
            Some("gcloud application default credentials")
        );
    }

    /// pi `:83`: an empty environment is "not configured" — `undefined`, not an error and not a
    /// keyless success.
    #[tokio::test]
    async fn an_empty_environment_is_not_configured() {
        assert!(resolve(&FakeCtx::new(), None).await.is_none());
    }
}
