//! The Amazon Bedrock provider (arch-01 §5) — a 1:1 port of pi v0.83.0
//! `packages/ai/src/providers/amazon-bedrock.ts`.
//!
//! Bedrock is the widest of the built-in catalogs (109 rows) and the only one whose credential is
//! usually *ambient*: apart from an explicit bearer token, every arm of
//! [`AmazonBedrockApiKeyAuth::resolve`] merely **detects** that the AWS SDK's default credential
//! chain will succeed and returns an [`AuthResult`] with no key at all
//! (`amazon-bedrock.ts:52-71`). Upstream's doc comment says so in as many words: "resolve also
//! detects ambient AWS credentials without copying them into pi's credential store"
//! (`amazon-bedrock.ts:6-10`).
//!
//! The precedence is a straight seven-step ladder, and the labels it hangs on each rung are
//! operator-visible, so they are reproduced verbatim:
//!
//! | pi line | condition | `source` |
//! |---|---|---|
//! | `:53` | stored `credential.key` | `stored credential` |
//! | `:56` | `AWS_BEARER_TOKEN_BEDROCK` | `AWS_BEARER_TOKEN_BEDROCK` |
//! | `:57` | credential `AWS_PROFILE`, else env `AWS_PROFILE` | `stored credential` / `AWS_PROFILE` |
//! | `:64` | `AWS_ACCESS_KEY_ID` **and** `AWS_SECRET_ACCESS_KEY` | `AWS access keys` |
//! | `:67` | `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI` | `ECS task role` |
//! | `:68` | `AWS_CONTAINER_CREDENTIALS_FULL_URI` | `ECS task role` |
//! | `:69` | `AWS_WEB_IDENTITY_TOKEN_FILE` | `web identity token` |
//!
//! Two JS operators in that ladder do not mean what the obvious Rust reading would, and both are
//! reproduced deliberately (see [`AmazonBedrockApiKeyAuth::resolve`]): the truthiness tests at
//! `:53`/`:56`/`:64`/`:67-69` drop `""`, and the `??` at `:57` lets a stored *empty* `AWS_PROFILE`
//! suppress the env lookup without satisfying the branch.
//!
//! This same ladder already exists once in this crate, in [`crate::env_api_keys::get_env_api_key`]
//! (`env-api-keys.ts:156`), which folds it into the `"<authenticated>"` sentinel used to answer
//! "is this provider configured at all?". That copy is a *predicate*; this one *resolves*, carrying
//! the env overlay and the source label a request needs. Upstream keeps the two apart the same way.
//!
//! # Catalog provenance — read before refreshing
//!
//! The 109 models in `catalog/amazon-bedrock.json` are the verbatim contents of pi
//! `packages/ai/src/providers/amazon-bedrock.models.ts` at commit `b0c2a90e` (2026-07-17), the LAST
//! revision at which pi tracks this catalog's literal data in git. One commit later (`a9f6a315`,
//! "feat(ai): separate generated model data") the data moved to
//! `packages/ai/src/providers/data/amazon-bedrock.json`, which `.gitignore:11` excludes — so at the
//! ported tag `v0.83.0` the catalog is not obtainable from the repository at all, and `b0c2a90e` is
//! the closest knowable snapshot to it. **Since 2026-08-15 that is true of EVERY embedded catalog,
//! not just this one:** all 35 are generated from `b0c2a90e` by
//! `cargo run -p xtask -- gen-catalogs` (PROV-018/PROV-060), so the "four newer files among 31
//! older ones" split this note used to describe is gone, and `catalog_manifest.json` records one
//! revision with a per-provider source map. Do not hand-edit this file — `gen-catalogs --check`
//! fails if you do.
//!
//! Unlike every other built-in, the Bedrock catalog carries **two** base URLs — the `eu.*`
//! inference profiles point at `eu-central-1`, everything else at `us-east-1` — and it is the one
//! catalog with no `compat` block on any row, so it cannot widen the tool-search blast radius
//! pinned by `api/anthropic_messages.rs`.
//!
//! # What is not here
//!
//! * **The wire api.** Every row's `api` is [`crate::known_api::BEDROCK_CONVERSE_STREAM`], whose
//!   impl is a separate module (`api/bedrock_converse_stream.rs`, upstream
//!   `packages/ai/src/api/bedrock-converse-stream.ts`, reached through the
//!   `bedrock-converse-stream.lazy.ts` shim at `amazon-bedrock.ts:1`) registered by
//!   `api/mod.rs`'s `register_builtins`. Nothing here depends on it: this module resolves the
//!   catalog and the auth ladder, and a registry that does not carry that impl simply fails the
//!   lookup with a terminal `StreamEvent::Error` (`wire.rs`, R-01-008/017/018) after auth has
//!   already resolved. That impl is also where AWS SigV4 signing lives — the ambient arms below
//!   return **no** key precisely because the signature, not a bearer, is what authenticates those
//!   requests.
//! * **`bedrockAuth.login`** (`amazon-bedrock.ts:13-51`) — the three-way interactive picker
//!   (bearer token / AWS profile / existing credential chain) that *writes* the credential,
//!   including the `notify` with the AWS credential-provider-chain doc link — IS ported below, on
//!   [`crate::auth::ApiKeyAuth::login`], the trait member CFG-005 added beside `name` + `resolve`
//!   (pi `ai/src/auth/types.ts:166`).

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::types::{AuthContext, AuthResult, Credential, ModelAuth, ProviderEnv};
use crate::auth::{ApiKeyAuth, CredentialStore, InMemoryCredentialStore, ProviderAuth};
use crate::error::AuthError;
use crate::model::Model;
use crate::wire::WireProvider;
use std::sync::Arc;

/// The provider id (pi `amazon-bedrock.ts:76`).
pub const AMAZON_BEDROCK_PROVIDER_ID: &str = "amazon-bedrock";

/// The display name (pi `amazon-bedrock.ts:77`).
pub const AMAZON_BEDROCK_PROVIDER_NAME: &str = "Amazon Bedrock";

/// The strategy label shown wherever auth is described (pi `bedrockAuth.name`,
/// `amazon-bedrock.ts:12`).
pub const AMAZON_BEDROCK_AUTH_NAME: &str = "AWS credentials or bearer token";

/// The `us-east-1` Bedrock runtime endpoint — the `baseUrl` of 100 of the 109 catalog rows (pi
/// `amazon-bedrock.models.ts`).
pub const BEDROCK_US_EAST_1_BASE_URL: &str = "https://bedrock-runtime.us-east-1.amazonaws.com";

/// The `eu-central-1` Bedrock runtime endpoint — the `baseUrl` of the nine `eu.*` inference
/// profiles (pi `amazon-bedrock.models.ts`).
pub const BEDROCK_EU_CENTRAL_1_BASE_URL: &str = "https://bedrock-runtime.eu-central-1.amazonaws.com";

/// The three `bedrockAuth.login` option ids (pi `amazon-bedrock.ts:18-20`).
const METHOD_BEARER_TOKEN: &str = "bearer-token";
const METHOD_AWS_PROFILE: &str = "aws-profile";
const METHOD_CREDENTIAL_CHAIN: &str = "credential-chain";

/// The `bedrockAuth.login` prompt/notify strings, verbatim from `amazon-bedrock.ts:16`, `:26`,
/// `:31`, `:35`, `:42` and `:48`.
const SELECT_AUTH_METHOD_MESSAGE: &str = "Select Amazon Bedrock authentication method:";
const ENTER_BEARER_TOKEN_MESSAGE: &str = "Enter Amazon Bedrock bearer token";
const CREDENTIAL_CHAIN_INFO_MESSAGE: &str =
    "Amazon Bedrock supports AWS profiles, IAM credentials, and role-based credentials.";
const AWS_CREDENTIAL_CHAIN_DOCS_URL: &str =
    "https://docs.aws.amazon.com/sdkref/latest/guide/standardized-credentials.html";
const ENTER_AWS_PROFILE_MESSAGE: &str = "Enter AWS profile name";
const CONFIGURE_THEN_CONTINUE_MESSAGE: &str =
    "Configure AWS credentials, then press Enter to continue";

/// A long-lived Bedrock API key, used as a bearer (pi `amazon-bedrock.ts:56`).
pub const AWS_BEARER_TOKEN_BEDROCK_ENV: &str = "AWS_BEARER_TOKEN_BEDROCK";

/// The named AWS profile in `~/.aws/config` (pi `amazon-bedrock.ts:57`).
pub const AWS_PROFILE_ENV: &str = "AWS_PROFILE";

/// Static IAM credentials, checked as a pair (pi `amazon-bedrock.ts:64`).
pub const AWS_ACCESS_KEY_ID_ENV: &str = "AWS_ACCESS_KEY_ID";

/// The secret half of the IAM pair (pi `amazon-bedrock.ts:64`).
pub const AWS_SECRET_ACCESS_KEY_ENV: &str = "AWS_SECRET_ACCESS_KEY";

/// ECS/Fargate task-role credential endpoint, relative form (pi `amazon-bedrock.ts:67`).
pub const AWS_CONTAINER_CREDENTIALS_RELATIVE_URI_ENV: &str =
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI";

/// ECS/EKS task-role credential endpoint, absolute form (pi `amazon-bedrock.ts:68`).
pub const AWS_CONTAINER_CREDENTIALS_FULL_URI_ENV: &str = "AWS_CONTAINER_CREDENTIALS_FULL_URI";

/// IRSA / OIDC web-identity token file (pi `amazon-bedrock.ts:69`).
pub const AWS_WEB_IDENTITY_TOKEN_FILE_ENV: &str = "AWS_WEB_IDENTITY_TOKEN_FILE";

/// `source` when the value came off the stored credential (pi `amazon-bedrock.ts:54`/`:61`).
const SOURCE_STORED_CREDENTIAL: &str = "stored credential";

/// `source` for the static IAM-pair arm (pi `amazon-bedrock.ts:65`).
const SOURCE_AWS_ACCESS_KEYS: &str = "AWS access keys";

/// `source` shared by BOTH container-credential arms (pi `amazon-bedrock.ts:67-68`).
const SOURCE_ECS_TASK_ROLE: &str = "ECS task role";

/// `source` for the web-identity arm (pi `amazon-bedrock.ts:69`).
const SOURCE_WEB_IDENTITY_TOKEN: &str = "web identity token";

/// The verbatim catalog extracted from pi's generated `amazon-bedrock.models.ts` (see the
/// module-level provenance note).
const AMAZON_BEDROCK_CATALOG_JSON: &str = include_str!("catalog/amazon-bedrock.json");

/// The full Bedrock catalog (1:1 with pi `AMAZON_BEDROCK_MODELS`, `amazon-bedrock.ts:79`). A parse
/// failure yields an empty catalog (surfaced loudly by the count test) rather than a panic
/// (NO-PANIC policy).
pub fn amazon_bedrock_models() -> Vec<Model> {
    serde_json::from_str(AMAZON_BEDROCK_CATALOG_JSON).unwrap_or_default()
}

/// The Bedrock [`ProviderAuth`] (pi `auth: { apiKey: bedrockAuth }`, `amazon-bedrock.ts:78`).
pub fn amazon_bedrock_auth() -> ProviderAuth {
    ProviderAuth::with_api_key(Arc::new(AmazonBedrockApiKeyAuth))
}

/// Construct the Bedrock provider over the given credential store + shared api registry (pi
/// `amazonBedrockProvider`, `amazon-bedrock.ts:74-82`).
///
/// The registry must provide the `bedrock-converse-stream` impl for a request to stream; see the
/// module note.
pub fn amazon_bedrock_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        AMAZON_BEDROCK_PROVIDER_ID,
        AMAZON_BEDROCK_PROVIDER_NAME,
        amazon_bedrock_models(),
        amazon_bedrock_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn amazon_bedrock_provider() -> WireProvider {
    amazon_bedrock_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
}

/// `credential?.key` — `undefined` for a missing credential, and for an OAuth one (pi types the
/// argument as `ApiKeyCredential`, so an OAuth credential simply has no `key` member).
fn credential_key(cred: Option<&Credential>) -> Option<&String> {
    match cred {
        Some(Credential::ApiKey { key, .. }) => key.as_ref(),
        _ => None,
    }
}

/// `credential?.env?.<name>` (pi `amazon-bedrock.ts:57`/`:61`).
fn credential_env_var<'a>(env: Option<&'a ProviderEnv>, name: &str) -> Option<&'a String> {
    env.and_then(|e| e.get(name))
}

/// JS truthiness for a `string | undefined`: `undefined` and `""` are both falsy.
fn truthy(value: Option<&String>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

/// An ambient arm: `{ auth: {}, source }` with no key and no env overlay (pi
/// `amazon-bedrock.ts:56`, `:65`, `:67-69`).
fn ambient(source: &str) -> Option<AuthResult> {
    Some(AuthResult {
        auth: ModelAuth::default(),
        env: None,
        source: Some(source.to_string()),
    })
}

/// pi `bedrockAuth` (`amazon-bedrock.ts:11-72`) — the `resolve` half.
pub struct AmazonBedrockApiKeyAuth;

#[async_trait::async_trait]
impl ApiKeyAuth for AmazonBedrockApiKeyAuth {
    /// pi `bedrockAuth.name` (`amazon-bedrock.ts:12`).
    fn name(&self) -> &str {
        AMAZON_BEDROCK_AUTH_NAME
    }

    fn supports_login(&self) -> bool {
        true
    }

    /// 1:1 port of `bedrockAuth.login` (`amazon-bedrock.ts:13-51`): a three-way picker, then a
    /// bearer-token secret, an AWS profile name, or a bare acknowledgement that the ambient
    /// credential chain is configured. An unknown option id is upstream's
    /// `Unknown Amazon Bedrock auth method: {method}` (`:45`).
    ///
    /// CFG-005: two of the three arms produce a credential with **no key** — one carries only
    /// `AWS_PROFILE`, one carries nothing at all and exists purely to record that the operator
    /// chose the ambient chain. A single-secret `/login` cannot express either.
    async fn login(
        &self,
        interaction: &dyn crate::auth::oauth::AuthInteraction,
    ) -> Result<Credential, crate::auth::oauth::OAuthError> {
        use crate::auth::oauth::{
            AuthEvent, AuthInfoLink, AuthPrompt, AuthSelectOption, OAuthError,
        };

        // `:14-22`
        let method = interaction
            .prompt(AuthPrompt::select(
                SELECT_AUTH_METHOD_MESSAGE,
                vec![
                    AuthSelectOption {
                        id: METHOD_BEARER_TOKEN.to_string(),
                        label: "Bearer token".to_string(),
                        description: None,
                    },
                    AuthSelectOption {
                        id: METHOD_AWS_PROFILE.to_string(),
                        label: "AWS profile".to_string(),
                        description: None,
                    },
                    AuthSelectOption {
                        id: METHOD_CREDENTIAL_CHAIN.to_string(),
                        label: "Existing AWS credential chain".to_string(),
                        description: None,
                    },
                ],
            ))
            .await?;

        // `:23-28`
        if method == METHOD_BEARER_TOKEN {
            let key = interaction
                .prompt(AuthPrompt::secret(ENTER_BEARER_TOKEN_MESSAGE))
                .await?;
            return Ok(Credential::ApiKey {
                key: Some(key),
                env: None,
            });
        }

        // `:29-38` — the notify fires for BOTH remaining arms, before the profile prompt.
        interaction.notify(AuthEvent::Info {
            message: CREDENTIAL_CHAIN_INFO_MESSAGE.to_string(),
            links: vec![AuthInfoLink {
                label: Some("AWS credential provider chain".to_string()),
                url: AWS_CREDENTIAL_CHAIN_DOCS_URL.to_string(),
            }],
        });

        // `:39-44`
        if method == METHOD_AWS_PROFILE {
            let profile = interaction
                .prompt(AuthPrompt::text(ENTER_AWS_PROFILE_MESSAGE))
                .await?;
            let mut env = ProviderEnv::new();
            env.insert(AWS_PROFILE_ENV.to_string(), profile);
            return Ok(Credential::ApiKey {
                key: None,
                env: Some(env),
            });
        }

        // `:45`
        if method != METHOD_CREDENTIAL_CHAIN {
            return Err(OAuthError::Failed(format!(
                "Unknown Amazon Bedrock auth method: {method}"
            )));
        }
        // `:46-49` — a TEXT prompt used purely as "press Enter to continue"; its answer is
        // discarded, but the await is what blocks until the operator has configured AWS.
        let _ = interaction
            .prompt(AuthPrompt::text(CONFIGURE_THEN_CONTINUE_MESSAGE))
            .await?;
        // `:50` — `{ type: "api_key" }`: no key, no env.
        Ok(Credential::ApiKey {
            key: None,
            env: None,
        })
    }

    /// pi `bedrockAuth.resolve` (`amazon-bedrock.ts:52-71`), rung for rung.
    ///
    /// Bedrock is not an `envApiKeyAuth`: six of its seven rungs return **no key at all**, because
    /// what authenticates those requests is an AWS SigV4 signature the wire impl computes from the
    /// ambient credential chain. Only the first two rungs carry a bearer.
    ///
    /// Two JS operators here do not mean what the obvious Rust reading would:
    ///
    /// * `if (value)` — truthiness, so `""` is absent (`:53`, `:56`, `:64`, `:67-69`). An env var
    ///   exported as the empty string does NOT configure the provider.
    /// * `??` — `credential?.env?.AWS_PROFILE ?? (await ctx.env("AWS_PROFILE"))` (`:57`). A stored
    ///   profile of `""` is not nullish, so it wins the coalesce and *suppresses* the env lookup;
    ///   the surrounding `if (…)` then fails on truthiness and resolution falls through to the IAM
    ///   pair. A stored empty profile therefore means "no profile", never "read the env".
    ///
    /// The `&&` at `:64` short-circuits, so `AWS_SECRET_ACCESS_KEY` is never read when
    /// `AWS_ACCESS_KEY_ID` is absent — Rust's `&&` short-circuits identically, including across the
    /// `.await`, so the number of [`AuthContext::env`] calls matches upstream too.
    async fn resolve(
        &self,
        _model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let cred_env = cred.and_then(Credential::env);

        // `if (credential?.key) return { auth: { apiKey: credential.key }, env: credential.env,
        //  source: "stored credential" };` (`:53-55`). This is the ONLY arm that forwards a stored
        // key, and it forwards the credential's env overlay with it.
        let cred_key = credential_key(cred);
        if truthy(cred_key) {
            return Ok(Some(AuthResult {
                auth: ModelAuth {
                    api_key: cred_key.cloned(),
                    ..Default::default()
                },
                env: cred_env.cloned(),
                source: Some(SOURCE_STORED_CREDENTIAL.to_string()),
            }));
        }

        // `if (await ctx.env("AWS_BEARER_TOKEN_BEDROCK")) return { auth: {}, source: … };` (`:56`).
        // Note upstream returns `auth: {}` — it does NOT put the bearer on the request here; the
        // wire impl reads the env var itself, exactly as the AWS SDK does.
        if truthy(ctx.env(AWS_BEARER_TOKEN_BEDROCK_ENV).await.as_ref()) {
            return Ok(ambient(AWS_BEARER_TOKEN_BEDROCK_ENV));
        }

        // `if (credential?.env?.AWS_PROFILE ?? (await ctx.env("AWS_PROFILE")))` (`:57-63`).
        let cred_profile = credential_env_var(cred_env, AWS_PROFILE_ENV);
        let profile = match cred_profile {
            // `??` is nullish, not falsy: a stored `""` short-circuits the env lookup.
            Some(stored) => Some(stored.clone()),
            None => ctx.env(AWS_PROFILE_ENV).await,
        };
        if truthy(profile.as_ref()) {
            return Ok(Some(AuthResult {
                auth: ModelAuth::default(),
                // `env: credential?.env` (`:60`) — the WHOLE overlay, even when the profile that
                // satisfied the branch came from the ambient env rather than the credential.
                env: cred_env.cloned(),
                // `credential?.env?.AWS_PROFILE ? "stored credential" : "AWS_PROFILE"` (`:61`).
                source: Some(
                    if truthy(cred_profile) {
                        SOURCE_STORED_CREDENTIAL
                    } else {
                        AWS_PROFILE_ENV
                    }
                    .to_string(),
                ),
            }));
        }

        // `if ((await ctx.env("AWS_ACCESS_KEY_ID")) && (await ctx.env("AWS_SECRET_ACCESS_KEY")))`
        // (`:64-66`) — both halves required, and the second is not read when the first is falsy.
        if truthy(ctx.env(AWS_ACCESS_KEY_ID_ENV).await.as_ref())
            && truthy(ctx.env(AWS_SECRET_ACCESS_KEY_ENV).await.as_ref())
        {
            return Ok(ambient(SOURCE_AWS_ACCESS_KEYS));
        }

        // `:67` and `:68` — two different env vars, ONE shared source label.
        if truthy(
            ctx.env(AWS_CONTAINER_CREDENTIALS_RELATIVE_URI_ENV)
                .await
                .as_ref(),
        ) {
            return Ok(ambient(SOURCE_ECS_TASK_ROLE));
        }
        if truthy(
            ctx.env(AWS_CONTAINER_CREDENTIALS_FULL_URI_ENV)
                .await
                .as_ref(),
        ) {
            return Ok(ambient(SOURCE_ECS_TASK_ROLE));
        }

        // `:69`
        if truthy(ctx.env(AWS_WEB_IDENTITY_TOKEN_FILE_ENV).await.as_ref()) {
            return Ok(ambient(SOURCE_WEB_IDENTITY_TOKEN));
        }

        // `return undefined` — not configured (`:70`).
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
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    // ------------------------------------------------------------------ env fixtures

    /// An [`AuthContext`] over a fixed map that RECORDS every lookup, so the short-circuit at
    /// `amazon-bedrock.ts:64` is observable and not merely assumed.
    struct MapEnv {
        vars: BTreeMap<String, String>,
        seen: Mutex<Vec<String>>,
    }

    impl MapEnv {
        fn new(vars: &[(&str, &str)]) -> Self {
            MapEnv {
                vars: vars
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
                seen: Mutex::new(Vec::new()),
            }
        }
        fn lookups(&self) -> Vec<String> {
            self.seen.lock().map(|g| g.clone()).unwrap_or_default()
        }
    }

    #[async_trait::async_trait]
    impl AuthContext for MapEnv {
        async fn env(&self, name: &str) -> Option<String> {
            if let Ok(mut g) = self.seen.lock() {
                g.push(name.to_string());
            }
            self.vars.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    fn a_model() -> Model {
        amazon_bedrock_models()
            .into_iter()
            .next()
            .expect("the catalog is non-empty")
    }

    /// Resolve against a fixed env + optional credential, returning the [`AuthResult`].
    async fn resolve_with(
        env: &[(&str, &str)],
        cred: Option<&Credential>,
    ) -> Option<AuthResult> {
        AmazonBedrockApiKeyAuth
            .resolve(&a_model(), &MapEnv::new(env), cred)
            .await
            .expect("resolve never errors upstream — it returns undefined")
    }

    /// The `source` label of a successful resolve.
    async fn source_of(env: &[(&str, &str)], cred: Option<&Credential>) -> Option<String> {
        resolve_with(env, cred).await.and_then(|r| r.source)
    }

    fn cred_with_env(pairs: &[(&str, &str)]) -> Credential {
        Credential::ApiKey {
            key: None,
            env: Some(
                pairs
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                    .collect(),
            ),
        }
    }

    // ------------------------------------------------------------------ catalog

    /// pi `AMAZON_BEDROCK_MODELS` @`b0c2a90e`: 109 rows, every one on the
    /// `bedrock-converse-stream` wire api and owned by `amazon-bedrock`.
    #[test]
    fn catalog_parses_verbatim_with_expected_count() {
        let models = amazon_bedrock_models();
        assert_eq!(models.len(), 109);
        assert!(
            models
                .iter()
                .all(|m| m.provider.as_str() == AMAZON_BEDROCK_PROVIDER_ID)
        );
        assert!(
            models
                .iter()
                .all(|m| m.api.as_str() == crate::known_api::BEDROCK_CONVERSE_STREAM)
        );
        // Ids are unique and non-empty, and every row has a real window.
        let mut ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        ids.sort_unstable();
        let unique = {
            let mut u = ids.clone();
            u.dedup();
            u.len()
        };
        assert_eq!(unique, 109, "duplicate model id in the Bedrock catalog");
        assert!(models.iter().all(|m| m.context_window > 0));
        assert!(models.iter().all(|m| m.max_tokens > 0));
        assert!(models.iter().all(|m| !m.base_url.is_empty()));
    }

    /// Bedrock is the ONLY built-in catalog with two endpoints: the nine `eu.*` inference profiles
    /// live in `eu-central-1`, everything else in `us-east-1` (pi `amazon-bedrock.models.ts`
    /// @`b0c2a90e`). A refresh that flattened them onto one host would silently cross-region every
    /// EU request.
    #[test]
    fn the_eu_inference_profiles_use_the_eu_central_1_endpoint() {
        let models = amazon_bedrock_models();
        let eu: Vec<&str> = models
            .iter()
            .filter(|m| m.base_url == BEDROCK_EU_CENTRAL_1_BASE_URL)
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(
            eu,
            vec![
                "eu.anthropic.claude-fable-5",
                "eu.anthropic.claude-haiku-4-5-20251001-v1:0",
                "eu.anthropic.claude-opus-4-5-20251101-v1:0",
                "eu.anthropic.claude-opus-4-6-v1",
                "eu.anthropic.claude-opus-4-7",
                "eu.anthropic.claude-opus-4-8",
                "eu.anthropic.claude-sonnet-4-5-20250929-v1:0",
                "eu.anthropic.claude-sonnet-4-6",
                "eu.anthropic.claude-sonnet-5",
            ]
        );
        // MIRROR: every OTHER row — including the `us.`/`jp.`/`au.`/`global.` profiles — is
        // us-east-1, so the assertion above pins the eu split rather than "some rows differ".
        assert!(
            models
                .iter()
                .filter(|m| !m.id.as_str().starts_with("eu."))
                .all(|m| m.base_url == BEDROCK_US_EAST_1_BASE_URL)
        );
        assert_eq!(
            models
                .iter()
                .filter(|m| m.base_url == BEDROCK_US_EAST_1_BASE_URL)
                .count(),
            100
        );
    }

    /// `us.anthropic.claude-opus-4-6-v1` verbatim from `amazon-bedrock.models.ts` @`b0c2a90e`. It is
    /// also the id `cyrup-config`'s `default_model_per_provider("amazon-bedrock")` names
    /// (`crates/cyrup-config/src/model.rs:938`), so a catalog that lost this row would leave the
    /// configured default unresolvable.
    #[test]
    fn opus_4_6_us_profile_matches_the_upstream_row() {
        let models = amazon_bedrock_models();
        let m = models
            .iter()
            .find(|m| m.id.as_str() == "us.anthropic.claude-opus-4-6-v1")
            .expect("us.anthropic.claude-opus-4-6-v1");
        assert_eq!(m.name, "Claude Opus 4.6 (US)");
        assert_eq!(m.base_url, BEDROCK_US_EAST_1_BASE_URL);
        assert!(m.reasoning);
        assert_eq!(m.input, vec![Modality::Text, Modality::Image]);
        assert_eq!(m.context_window, 1_000_000);
        assert_eq!(m.max_tokens, 128_000);
        // Real, distinct, non-zero rates — Bedrock's Opus 4.6 pricing, not a defaulted cost block.
        assert_eq!(m.cost.input, 5.0);
        assert_eq!(m.cost.output, 25.0);
        assert_eq!(m.cost.cache_read, 0.5);
        assert_eq!(m.cost.cache_write, 6.25);
        assert!(m.cost.tiers.is_none(), "Bedrock ships no long-context tiers");
        let map = m.thinking_level_map.as_ref().expect("thinkingLevelMap");
        assert_eq!(map.get("max"), Some(&Some("max".to_string())));
        assert_eq!(map.get("xhigh"), None, "4.6 has no native xhigh rung");
        assert_eq!(map.len(), 1);
    }

    /// A MIRROR row of a completely different shape: `deepseek.v3-v1:0` is text-only, has no
    /// thinking map at all, and prices cache reads/writes at zero. Asserted so the row above pins
    /// per-model data rather than a uniform catalog.
    #[test]
    fn deepseek_v3_is_the_odd_row_out() {
        let models = amazon_bedrock_models();
        let m = models
            .iter()
            .find(|m| m.id.as_str() == "deepseek.v3-v1:0")
            .expect("deepseek.v3-v1:0");
        assert_eq!(m.name, "DeepSeek-V3.1");
        assert_eq!(m.input, vec![Modality::Text]);
        assert!(!m.supports_image_input());
        assert!(m.thinking_level_map.is_none());
        assert_eq!(m.cost.input, 0.58);
        assert_eq!(m.cost.output, 1.68);
        assert_eq!(m.cost.cache_read, 0.0);
        assert_eq!(m.cost.cache_write, 0.0);
        assert_eq!(m.context_window, 163_840);
        assert_eq!(m.max_tokens, 81_920);

        // 29 of the 109 rows are non-reasoning, and 37 are text-only — the catalog is genuinely
        // heterogeneous.
        assert_eq!(models.iter().filter(|m| !m.reasoning).count(), 29);
        assert_eq!(
            models.iter().filter(|m| m.supports_image_input()).count(),
            72
        );
    }

    /// Bedrock is the one catalog with **no** `compat` block anywhere, so it cannot leak
    /// `supportsToolSearch` into the blast radius pinned by
    /// `api/anthropic_messages.rs::tool_search_is_confined_to_the_openai_responses_catalog`.
    #[test]
    fn no_bedrock_row_carries_a_compat_block() {
        assert!(amazon_bedrock_models().iter().all(|m| m.compat.is_none()));
    }

    /// pi `fbdd4638` added the `max` rung; the Bedrock catalog picks it up on the Claude profiles
    /// and `xhigh` on the OpenAI ones — and NEVER as the defective `{"xhigh":"max"}` remap that
    /// `tests/thinking_max.rs::no_catalog_still_remaps_xhigh_onto_max` forbids.
    #[test]
    fn thinking_level_maps_match_upstream_and_never_remap_xhigh_onto_max() {
        let models = amazon_bedrock_models();
        let with_map: Vec<&Model> = models
            .iter()
            .filter(|m| m.thinking_level_map.is_some())
            .collect();
        assert_eq!(with_map.len(), 37);
        for m in &with_map {
            let map = m.thinking_level_map.as_ref().expect("checked above");
            assert_ne!(
                map.get("xhigh"),
                Some(&Some("max".to_string())),
                "{} relabels the `max` effort as `xhigh`",
                m.id.as_str()
            );
        }

        // `anthropic.claude-fable-5`: the only shape carrying an explicit `"off": null` alongside
        // both top rungs.
        let fable = models
            .iter()
            .find(|m| m.id.as_str() == "anthropic.claude-fable-5")
            .expect("anthropic.claude-fable-5");
        let map = fable.thinking_level_map.as_ref().expect("map");
        assert_eq!(map.get("off"), Some(&None));
        assert_eq!(map.get("xhigh"), Some(&Some("xhigh".to_string())));
        assert_eq!(map.get("max"), Some(&Some("max".to_string())));

        // MIRROR: the OpenAI profiles stop at `xhigh` — no `max` rung on Bedrock's GPT rows.
        let terra = models
            .iter()
            .find(|m| m.id.as_str() == "openai.gpt-5.6-terra")
            .expect("openai.gpt-5.6-terra");
        let map = terra.thinking_level_map.as_ref().expect("map");
        assert_eq!(map.get("xhigh"), Some(&Some("xhigh".to_string())));
        assert_eq!(map.get("max"), None);
    }

    // ------------------------------------------------------------------ provider shape

    /// pi `amazonBedrockProvider` (`amazon-bedrock.ts:74-82`): id, display name, the full catalog,
    /// and api-key auth only — there is no OAuth strategy for Bedrock.
    #[test]
    fn provider_matches_the_upstream_factory() {
        let provider = amazon_bedrock_provider();
        assert_eq!(provider.id().as_str(), "amazon-bedrock");
        assert_eq!(provider.name(), "Amazon Bedrock");
        assert_eq!(provider.models().len(), 109);

        let auth = provider.provider_auth().expect("bedrock declares auth");
        assert!(auth.api_key.is_some());
        assert!(
            auth.oauth.is_none(),
            "pi wires `auth: {{ apiKey: bedrockAuth }}` only (amazon-bedrock.ts:78)"
        );
        assert_eq!(
            auth.api_key.as_ref().map(|a| a.name().to_string()),
            Some("AWS credentials or bearer token".to_string())
        );
    }

    // ------------------------------------------------------------------ resolve: the ladder

    /// Each rung in isolation, with the exact `source` label upstream hangs on it
    /// (`amazon-bedrock.ts:52-70`). The two ECS vars deliberately share one label.
    #[tokio::test]
    async fn every_ambient_rung_resolves_with_its_upstream_source_label() {
        for (var, source) in [
            ("AWS_BEARER_TOKEN_BEDROCK", "AWS_BEARER_TOKEN_BEDROCK"),
            ("AWS_PROFILE", "AWS_PROFILE"),
            ("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "ECS task role"),
            ("AWS_CONTAINER_CREDENTIALS_FULL_URI", "ECS task role"),
            ("AWS_WEB_IDENTITY_TOKEN_FILE", "web identity token"),
        ] {
            let result = resolve_with(&[(var, "set")], None)
                .await
                .unwrap_or_else(|| panic!("{var} must configure Bedrock"));
            assert_eq!(result.source.as_deref(), Some(source), "{var}");
            // Ambient arms carry NO key — SigV4/the SDK authenticates, not a bearer (`auth: {}`).
            assert!(result.auth.api_key.is_none(), "{var}");
            assert!(result.auth.base_url.is_none(), "{var}");
            assert!(result.auth.headers.is_none(), "{var}");
        }

        // The IAM pair is the one rung needing two vars.
        let pair = resolve_with(
            &[("AWS_ACCESS_KEY_ID", "AKIA"), ("AWS_SECRET_ACCESS_KEY", "s")],
            None,
        )
        .await
        .expect("the IAM pair configures Bedrock");
        assert_eq!(pair.source.as_deref(), Some("AWS access keys"));
        assert!(pair.auth.api_key.is_none());
    }

    /// `:70` — an env with none of the seven sources leaves the provider unconfigured, and an
    /// *empty* value never counts (`if (value)` is truthiness, not presence).
    #[tokio::test]
    async fn an_unconfigured_env_resolves_to_nothing() {
        assert!(resolve_with(&[], None).await.is_none());
        assert!(
            resolve_with(&[("AWS_REGION", "us-east-1")], None)
                .await
                .is_none(),
            "AWS_REGION alone is not a credential"
        );
        for var in [
            "AWS_BEARER_TOKEN_BEDROCK",
            "AWS_PROFILE",
            "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            "AWS_WEB_IDENTITY_TOKEN_FILE",
        ] {
            assert!(
                resolve_with(&[(var, "")], None).await.is_none(),
                "{var}=\"\" is falsy upstream and must not configure Bedrock"
            );
            // MIRROR: the same var with a value does configure it.
            assert!(resolve_with(&[(var, "x")], None).await.is_some(), "{var}");
        }
    }

    /// `:64` — the IAM pair needs BOTH halves, and `&&` short-circuits, so the secret is never even
    /// read when the id is absent.
    #[tokio::test]
    async fn the_iam_pair_requires_both_halves_and_short_circuits() {
        assert!(
            resolve_with(&[("AWS_ACCESS_KEY_ID", "AKIA")], None)
                .await
                .is_none()
        );
        assert!(
            resolve_with(&[("AWS_SECRET_ACCESS_KEY", "s")], None)
                .await
                .is_none()
        );
        // An empty id is falsy → the pair fails even with a real secret.
        assert!(
            resolve_with(&[("AWS_ACCESS_KEY_ID", ""), ("AWS_SECRET_ACCESS_KEY", "s")], None)
                .await
                .is_none()
        );

        // The short-circuit itself: with no id in the env, `AWS_SECRET_ACCESS_KEY` is not looked up.
        let env = MapEnv::new(&[]);
        let _ = AmazonBedrockApiKeyAuth
            .resolve(&a_model(), &env, None)
            .await
            .expect("resolve");
        let seen = env.lookups();
        assert!(seen.iter().any(|n| n == AWS_ACCESS_KEY_ID_ENV));
        assert!(
            !seen.iter().any(|n| n == AWS_SECRET_ACCESS_KEY_ENV),
            "`&&` must short-circuit as upstream's does, saw {seen:?}"
        );
        // MIRROR: with the id present the secret IS read.
        let env = MapEnv::new(&[("AWS_ACCESS_KEY_ID", "AKIA")]);
        let _ = AmazonBedrockApiKeyAuth
            .resolve(&a_model(), &env, None)
            .await
            .expect("resolve");
        assert!(
            env.lookups().iter().any(|n| n == AWS_SECRET_ACCESS_KEY_ENV),
            "the secret must be read once the id is truthy"
        );
    }

    /// The ladder's ORDER (`:53` → `:56` → `:57` → `:64` → `:67` → `:68` → `:69`): with every source
    /// present at once, the earliest rung wins, and removing it hands off to the next.
    #[tokio::test]
    async fn the_ladder_is_ordered_and_each_rung_yields_to_the_one_above() {
        const ALL: [(&str, &str); 6] = [
            ("AWS_BEARER_TOKEN_BEDROCK", "bearer"),
            ("AWS_PROFILE", "prod"),
            ("AWS_ACCESS_KEY_ID", "AKIA"),
            ("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "/v2/creds"),
            ("AWS_CONTAINER_CREDENTIALS_FULL_URI", "http://169.254.170.2"),
            ("AWS_WEB_IDENTITY_TOKEN_FILE", "/var/run/token"),
        ];
        // A stored key beats everything ambient (`:53`).
        assert_eq!(
            source_of(&ALL, Some(&Credential::api_key("bedrock-key"))).await,
            Some("stored credential".to_string())
        );
        // …then the bearer token, then the profile, then the IAM pair, then the two ECS forms, then
        // web identity. Each step drops the winner and re-resolves.
        let expected = [
            "AWS_BEARER_TOKEN_BEDROCK",
            "AWS_PROFILE",
            "AWS access keys",
            "ECS task role",
            "ECS task role",
            "web identity token",
        ];
        for (dropped, source) in expected.iter().enumerate() {
            let mut env: Vec<(&str, &str)> = ALL.to_vec();
            env.drain(..dropped);
            // The IAM rung needs its second half present for that step.
            env.push(("AWS_SECRET_ACCESS_KEY", "secret"));
            assert_eq!(
                source_of(&env, None).await,
                Some((*source).to_string()),
                "after dropping the first {dropped} source(s)"
            );
        }
    }

    // ------------------------------------------------------------------ resolve: the credential

    /// `:53-55` — the stored key is the only value ever placed on the request, and the credential's
    /// env overlay rides along with it.
    #[tokio::test]
    async fn a_stored_key_is_returned_with_the_credential_env_overlay() {
        let cred = Credential::ApiKey {
            key: Some("bedrock-bearer".to_string()),
            env: Some(
                [("AWS_REGION".to_string(), "eu-central-1".to_string())]
                    .into_iter()
                    .collect(),
            ),
        };
        let result = resolve_with(&[], Some(&cred))
            .await
            .expect("a stored key configures Bedrock");
        assert_eq!(result.auth.api_key.as_deref(), Some("bedrock-bearer"));
        assert_eq!(result.source.as_deref(), Some("stored credential"));
        assert_eq!(
            result.env.as_ref().and_then(|e| e.get("AWS_REGION")),
            Some(&"eu-central-1".to_string())
        );

        // MIRROR / JS truthiness: an EMPTY stored key is falsy, so resolution falls through — and
        // with nothing ambient it reports "not configured" rather than an empty bearer.
        let empty = Credential::ApiKey {
            key: Some(String::new()),
            env: None,
        };
        assert!(resolve_with(&[], Some(&empty)).await.is_none());
        assert_eq!(
            source_of(&[("AWS_PROFILE", "prod")], Some(&empty)).await,
            Some("AWS_PROFILE".to_string()),
            "an empty stored key must not short-circuit the ladder"
        );
    }

    /// `:57-62` — a stored `AWS_PROFILE` satisfies the profile rung and relabels the source, and the
    /// whole credential env is forwarded either way.
    #[tokio::test]
    async fn a_stored_profile_relabels_the_source_and_forwards_the_env() {
        let cred = cred_with_env(&[("AWS_PROFILE", "stored-prof"), ("AWS_REGION", "us-west-2")]);
        let result = resolve_with(&[], Some(&cred))
            .await
            .expect("a stored profile configures Bedrock");
        assert_eq!(result.source.as_deref(), Some("stored credential"));
        assert!(result.auth.api_key.is_none(), "the profile arm carries no key");
        let env = result.env.as_ref().expect("credential env forwarded");
        assert_eq!(env.get("AWS_PROFILE"), Some(&"stored-prof".to_string()));
        assert_eq!(env.get("AWS_REGION"), Some(&"us-west-2".to_string()));

        // `env: credential?.env` (`:60`) is returned even when the AMBIENT profile is what satisfied
        // the branch — the label then says `AWS_PROFILE`, not `stored credential`.
        let cred = cred_with_env(&[("AWS_REGION", "us-west-2")]);
        let result = resolve_with(&[("AWS_PROFILE", "ambient")], Some(&cred))
            .await
            .expect("the ambient profile configures Bedrock");
        assert_eq!(result.source.as_deref(), Some("AWS_PROFILE"));
        assert_eq!(
            result.env.as_ref().and_then(|e| e.get("AWS_REGION")),
            Some(&"us-west-2".to_string())
        );
    }

    /// `:57`'s `??` is NULLISH, not falsy. A stored `AWS_PROFILE: ""` is not nullish, so it wins the
    /// coalesce and suppresses the env read; the surrounding truthiness test then fails and
    /// resolution falls through to the IAM pair. This is the sharp edge that distinguishes `??`
    /// from `||`, and getting it wrong would silently promote an ambient profile the operator
    /// explicitly blanked.
    #[tokio::test]
    async fn a_stored_empty_profile_suppresses_the_ambient_profile() {
        let cred = cred_with_env(&[("AWS_PROFILE", "")]);
        // MIRROR: with NO stored profile at all, the ambient one wins.
        assert_eq!(
            source_of(&[("AWS_PROFILE", "ambient")], None).await,
            Some("AWS_PROFILE".to_string())
        );
        // With the stored empty one, the ambient profile is not consulted and the ladder continues.
        assert!(
            resolve_with(&[("AWS_PROFILE", "ambient")], Some(&cred))
                .await
                .is_none(),
            "a blanked stored profile must not fall back to the ambient one"
        );
        assert_eq!(
            source_of(
                &[
                    ("AWS_PROFILE", "ambient"),
                    ("AWS_ACCESS_KEY_ID", "AKIA"),
                    ("AWS_SECRET_ACCESS_KEY", "s"),
                ],
                Some(&cred),
            )
            .await,
            Some("AWS access keys".to_string()),
            "resolution continues past the suppressed profile rung"
        );

        // And the env var really is never read on that path.
        let env = MapEnv::new(&[("AWS_PROFILE", "ambient")]);
        let _ = AmazonBedrockApiKeyAuth
            .resolve(&a_model(), &env, Some(&cred))
            .await
            .expect("resolve");
        assert!(
            !env.lookups().iter().any(|n| n == AWS_PROFILE_ENV),
            "`??` must short-circuit the env lookup, saw {:?}",
            env.lookups()
        );
    }

    /// An OAuth credential has no `key` and no `env` upstream (pi types `resolve`'s argument as
    /// `ApiKeyCredential`), so it neither satisfies nor blocks any rung.
    #[tokio::test]
    async fn an_oauth_credential_neither_satisfies_nor_blocks_a_rung() {
        let cred = Credential::Oauth {
            refresh: "rt".to_string(),
            access: "at".to_string(),
            expires: 0,
            ext: serde_json::Map::new(),
        };
        assert!(resolve_with(&[], Some(&cred)).await.is_none());
        let result = resolve_with(&[("AWS_PROFILE", "ambient")], Some(&cred))
            .await
            .expect("the ambient profile still resolves");
        assert_eq!(result.source.as_deref(), Some("AWS_PROFILE"));
        assert!(result.env.is_none());
    }
}
