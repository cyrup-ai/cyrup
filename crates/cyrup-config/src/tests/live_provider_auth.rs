//! The strategy-driven auth helpers an EXTENSION provider needs (`login.rs`: `live_provider_auth_*`,
//! `resolve_provider_auth_for`, `StoreAuthContext`) — pi's `checkAuth`, `getProviderAuthStatus` and
//! `getAuth` for a provider that is not in the built-in id table.
//!
//! The strategy under test is shaped like pi's llama.cpp one (`extensions/llama/provider.ts:156-196`
//! @v0.99.2-17): `check` is active only when a server URL is known — from the stored credential's
//! `env.LLAMA_BASE_URL` or from the `LLAMA_BASE_URL` environment variable — and `resolve` answers
//! `{ auth: { apiKey, baseUrl: <url>/v1 }, env: { …credential.env, LLAMA_BASE_URL } }`. It is a local
//! stub because `cyrup-config` cannot depend on `cyrup-llama`; the helpers under test see only the
//! `ApiKeyAuth` trait either way.
//!
//! **No network.** The strategy resolves strings; nothing here opens a socket.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::sync::Arc;

use cyrup_core::{EventStream, ProviderId};
use cyrup_provider::auth::{ApiKeyAuth, ProviderAuth};
use cyrup_provider::collection::{AuthCheck, AuthType};
use cyrup_provider::{
    AuthContext, AuthError, AuthResult, Context, Credential, Model, ModelAuth, Provider,
    StreamEvent, StreamOptions,
};

use crate::auth::{AuthSource, AuthStore, Credential as StoredCredential};
use crate::login::{
    StoreAuthContext, live_provider_auth_check, live_provider_auth_status,
    resolve_provider_auth_for,
};
use crate::test_util::{TempDir, temp_dir};

const ID: &str = "llama.cpp";
const URL_ENV: &str = "LLAMA_BASE_URL";

/// pi `credentialServerUrl` then `ctx.env("LLAMA_BASE_URL")` (`provider.ts:24-35`).
async fn server_url(ctx: &dyn AuthContext, cred: Option<&Credential>) -> Option<String> {
    let stored = cred
        .and_then(Credential::env)
        .and_then(|env| env.get(URL_ENV))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    match stored {
        Some(url) => Some(url),
        None => ctx.env(URL_ENV).await.map(|v| v.trim().to_string()),
    }
}

fn source_of(cred: Option<&Credential>) -> String {
    if cred.is_some() {
        "stored credential".to_string()
    } else {
        URL_ENV.to_string()
    }
}

struct UrlGatedKey;

#[async_trait::async_trait]
impl ApiKeyAuth for UrlGatedKey {
    fn name(&self) -> &str {
        "llama.cpp server"
    }
    fn supports_check(&self) -> bool {
        true
    }
    async fn check(
        &self,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthCheck>, AuthError> {
        Ok(server_url(ctx, cred).await.map(|_| AuthCheck {
            auth_type: AuthType::ApiKey,
            source: Some(source_of(cred)),
        }))
    }
    async fn resolve(
        &self,
        _model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let Some(url) = server_url(ctx, cred).await else {
            return Ok(None);
        };
        let key = match cred {
            Some(Credential::ApiKey { key: Some(key), .. }) => key.clone(),
            _ => ctx
                .env("LLAMA_API_KEY")
                .await
                .unwrap_or_else(|| "local".to_string()),
        };
        let mut env = cred.and_then(Credential::env).cloned().unwrap_or_default();
        env.insert(URL_ENV.to_string(), url.clone());
        Ok(Some(AuthResult {
            auth: ModelAuth {
                api_key: Some(key),
                headers: None,
                base_url: Some(format!("{url}/v1")),
            },
            env: Some(env),
            source: Some(source_of(cred)),
        }))
    }
}

struct LiveStub {
    id: ProviderId,
    auth: Option<ProviderAuth>,
}

#[async_trait::async_trait]
impl Provider for LiveStub {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &[]
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        self.auth.as_ref()
    }
    fn stream(
        &self,
        _model: &Model,
        _context: &Context,
        _options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let (_sink, stream) =
            cyrup_core::finalizing_channel::<StreamEvent, ()>(|_| true, |_| (), || ());
        Box::pin(stream)
    }
}

fn provider() -> Arc<dyn Provider> {
    Arc::new(LiveStub {
        id: ProviderId::from(ID),
        auth: Some(ProviderAuth::with_api_key(Arc::new(UrlGatedKey))),
    })
}

/// A store over a temp `auth.json` whose ambient environment is `env` and nothing else, so no test
/// reads (or depends on) the machine's real `LLAMA_BASE_URL`.
fn store(env: &[(&str, &str)]) -> (TempDir, Arc<AuthStore>) {
    let dir = temp_dir();
    let ambient: HashMap<String, String> = env
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let store = AuthStore::at(dir.join("auth.json")).with_ambient_env(ambient);
    (dir, Arc::new(store))
}

async fn store_credential(store: &AuthStore, key: Option<&str>, url: Option<&str>) {
    let env = url.map(|u| {
        let mut m = std::collections::BTreeMap::new();
        m.insert(URL_ENV.to_string(), u.to_string());
        m
    });
    let credential = StoredCredential::ApiKey {
        key: key.map(str::to_string),
        env,
    };
    store
        .modify(
            &ProviderId::from(ID),
            |_| async move { Ok(Some(credential)) },
        )
        .await
        .unwrap();
}

/// `check` is the strategy's: with no stored credential and no `LLAMA_BASE_URL`, the provider is
/// not configured — pi's "active only when a URL is known" (`provider.ts:181-186`).
#[tokio::test]
async fn check_is_none_without_a_credential_or_the_env_var() {
    let (_dir, store) = store(&[]);
    assert_eq!(
        live_provider_auth_check(&store, &provider()).await.unwrap(),
        None
    );
}

/// The environment tier is the STORE's ambient tier: the strategy's `ctx.env(LLAMA_BASE_URL)` sees
/// it, and the check names it as the source.
#[tokio::test]
async fn check_reads_the_url_from_the_store_ambient_environment() {
    let (_dir, store) = store(&[(URL_ENV, "http://127.0.0.1:9")]);
    let check = live_provider_auth_check(&store, &provider())
        .await
        .unwrap()
        .expect("configured by the env var");
    assert_eq!(check.auth_type, AuthType::ApiKey);
    assert_eq!(check.source.as_deref(), Some(URL_ENV));
}

/// A stored credential whose `env.LLAMA_BASE_URL` names the server configures the provider, and
/// the check names the credential as the source.
#[tokio::test]
async fn check_reads_the_url_from_the_stored_credential() {
    let (_dir, store) = store(&[]);
    store_credential(&store, Some("sk"), Some("http://127.0.0.1:9")).await;
    let check = live_provider_auth_check(&store, &provider())
        .await
        .unwrap()
        .expect("configured by the stored credential");
    assert_eq!(check.source.as_deref(), Some("stored credential"));
}

/// The strategy is AUTHORITATIVE: a stored credential that carries no URL, with no env var, does not
/// make the provider configured — which a generic "any stored credential counts" predicate
/// (`AuthStore::has_auth`) would have claimed.
#[tokio::test]
async fn a_stored_credential_without_a_url_is_not_configured() {
    let (_dir, store) = store(&[]);
    store_credential(&store, Some("sk"), None).await;
    assert!(
        store.has_auth(&ProviderId::from(ID), None),
        "the generic predicate says yes"
    );
    assert_eq!(
        live_provider_auth_check(&store, &provider()).await.unwrap(),
        None,
        "the strategy says no"
    );
}

/// A provider with no `auth` at all contributes nothing (`getProvider(id)?.auth` absent).
#[tokio::test]
async fn a_provider_without_an_auth_strategy_is_unconfigured() {
    let (_dir, store) = store(&[(URL_ENV, "http://127.0.0.1:9")]);
    let bare: Arc<dyn Provider> = Arc::new(LiveStub {
        id: ProviderId::from(ID),
        auth: None,
    });
    assert_eq!(live_provider_auth_check(&store, &bare).await.unwrap(), None);
    assert!(
        resolve_provider_auth_for(&store, bare.as_ref())
            .await
            .unwrap()
            .is_none()
    );
}

/// `getProviderAuthStatus` (`model-runtime.ts:428-437`): nothing → unconfigured; a stored credential
/// → `stored`; only the strategy's environment arm → `environment` labelled with the check's source.
#[tokio::test]
async fn status_follows_pis_runtime_stored_environment_order() {
    let (_dir, empty) = store(&[]);
    let status = live_provider_auth_status(&empty, &provider()).await;
    assert!(!status.configured);
    assert_eq!(status.source, None);

    let (_dir, env_only) = store(&[(URL_ENV, "http://127.0.0.1:9")]);
    let status = live_provider_auth_status(&env_only, &provider()).await;
    assert!(status.configured);
    assert_eq!(status.source, Some(AuthSource::Environment));
    assert_eq!(status.label.as_deref(), Some(URL_ENV));

    let (_dir, stored) = store(&[(URL_ENV, "http://127.0.0.1:9")]);
    store_credential(&stored, None, Some("http://127.0.0.1:9")).await;
    let status = live_provider_auth_status(&stored, &provider()).await;
    assert!(status.configured);
    assert_eq!(
        status.source,
        Some(AuthSource::Stored),
        "a stored credential outranks the environment arm"
    );
}

/// The runtime `--api-key` tier comes first, as pi's `hasRuntimeApiKey` does.
#[tokio::test]
async fn status_reports_a_runtime_api_key_first() {
    let (_dir, store) = store(&[]);
    store.set_runtime_api_key(ProviderId::from(ID), "from-the-flag".to_string());
    let status = live_provider_auth_status(&store, &provider()).await;
    assert!(status.configured);
    assert_eq!(status.source, Some(AuthSource::Runtime));
}

/// `getAuth(providerId)`: the stored credential's key and URL come back, the base URL is the
/// strategy's `<url>/v1`, and the env overlay carries `LLAMA_BASE_URL` — what `/llama` reads
/// (`extensions/llama/index.ts:30-37`).
#[tokio::test]
async fn resolve_returns_the_strategy_auth_for_a_stored_credential() {
    let (_dir, store) = store(&[]);
    store_credential(&store, Some("secret"), Some("http://127.0.0.1:9")).await;
    let result = resolve_provider_auth_for(&store, provider().as_ref())
        .await
        .unwrap()
        .expect("configured");
    assert_eq!(result.auth.api_key.as_deref(), Some("secret"));
    assert_eq!(
        result.auth.base_url.as_deref(),
        Some("http://127.0.0.1:9/v1")
    );
    assert_eq!(
        result
            .env
            .as_ref()
            .and_then(|e| e.get(URL_ENV))
            .map(String::as_str),
        Some("http://127.0.0.1:9")
    );
    assert_eq!(result.source.as_deref(), Some("stored credential"));
}

/// With nothing stored the strategy falls back to the ambient environment (pi
/// `resolve({ ctx, credential: undefined })`), and the key defaults to `"local"`.
#[tokio::test]
async fn resolve_falls_back_to_the_ambient_environment() {
    let (_dir, store) = store(&[(URL_ENV, "http://127.0.0.1:7")]);
    let result = resolve_provider_auth_for(&store, provider().as_ref())
        .await
        .unwrap()
        .expect("configured by the env var");
    assert_eq!(result.auth.api_key.as_deref(), Some("local"));
    assert_eq!(
        result.auth.base_url.as_deref(),
        Some("http://127.0.0.1:7/v1")
    );
    assert_eq!(result.source.as_deref(), Some(URL_ENV));
}

/// Unconfigured is `None`, pi's `undefined`.
#[tokio::test]
async fn resolve_is_none_when_unconfigured() {
    let (_dir, store) = store(&[]);
    assert!(
        resolve_provider_auth_for(&store, provider().as_ref())
            .await
            .unwrap()
            .is_none()
    );
}

/// A blank environment value is absent (pi `auth/context.ts:24-25`) and a real one is returned
/// untrimmed.
#[tokio::test]
async fn the_store_context_treats_a_blank_env_value_as_absent() {
    let (_dir, store) = store(&[("BLANK", "   "), ("PADDED", "  v  ")]);
    let ctx = StoreAuthContext(store);
    assert_eq!(ctx.env("BLANK").await, None);
    assert_eq!(ctx.env("MISSING").await, None);
    assert_eq!(ctx.env("PADDED").await.as_deref(), Some("  v  "));
}

// ------------------------------------------------------------------- failing check, resolve subject

/// A strategy whose `check` fails (pi's `apiKey.check` throwing).
struct FailingCheck;

#[async_trait::async_trait]
impl ApiKeyAuth for FailingCheck {
    fn name(&self) -> &str {
        "failing"
    }
    fn supports_check(&self) -> bool {
        true
    }
    async fn check(
        &self,
        _ctx: &dyn AuthContext,
        _cred: Option<&Credential>,
    ) -> Result<Option<AuthCheck>, AuthError> {
        Err(AuthError::api_key(
            ProviderId::from(ID),
            "the server is unreachable".to_string(),
        ))
    }
    async fn resolve(
        &self,
        _model: &Model,
        _ctx: &dyn AuthContext,
        _cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        Ok(None)
    }
}

/// pi's `checkAuth` rethrows a failing check as `ModelsError("auth", …)`; `getProviderAuthStatus`
/// records it and leaves the provider out of `snapshot.auth`, so the status line reads
/// UNCONFIGURED rather than claiming a source the strategy could not confirm.
#[tokio::test]
async fn a_failing_check_propagates_and_reads_as_unconfigured_in_the_status() {
    let (_dir, store) = store(&[(URL_ENV, "http://127.0.0.1:9")]);
    let failing: Arc<dyn Provider> = Arc::new(LiveStub {
        id: ProviderId::from(ID),
        auth: Some(ProviderAuth::with_api_key(Arc::new(FailingCheck))),
    });

    assert!(
        live_provider_auth_check(&store, &failing).await.is_err(),
        "the check failure propagates"
    );
    let status = live_provider_auth_status(&store, &failing).await;
    assert!(!status.configured, "{status:?}");
    assert_eq!(status.source, None);
    assert_eq!(status.label, None);
}

/// A strategy that records the model `resolve` was handed (cyrup's `resolve` takes a model; pi's
/// is provider-scoped) and answers `None`.
struct RecordsSubject {
    seen: Arc<std::sync::Mutex<Vec<Model>>>,
}

#[async_trait::async_trait]
impl ApiKeyAuth for RecordsSubject {
    fn name(&self) -> &str {
        "records"
    }
    async fn resolve(
        &self,
        model: &Model,
        _ctx: &dyn AuthContext,
        _cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        self.seen.lock().unwrap().push(model.clone());
        Ok(None)
    }
}

/// A live provider carrying `models` as its catalog, with the recording strategy.
struct CatalogStub {
    id: ProviderId,
    auth: ProviderAuth,
    models: Vec<Model>,
}

#[async_trait::async_trait]
impl Provider for CatalogStub {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &self.models
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        Some(&self.auth)
    }
    fn stream(
        &self,
        _model: &Model,
        _context: &Context,
        _options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let (_sink, stream) =
            cyrup_core::finalizing_channel::<StreamEvent, ()>(|_| true, |_| (), || ());
        Box::pin(stream)
    }
}

/// The model `resolve` is handed: the provider's FIRST catalog row when it has one (Cloudflare reads
/// `model.base_url`), and the identity-only probe model naming the provider when its catalog has not
/// been listed yet (llama.cpp before its first `/llama` list).
#[tokio::test]
async fn resolve_is_handed_the_first_catalog_row_or_a_probe_naming_the_provider() {
    let (_dir, store) = store(&[]);
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let auth = ProviderAuth::with_api_key(Arc::new(RecordsSubject {
        seen: Arc::clone(&seen),
    }));

    // No catalog yet: the probe model.
    let empty = CatalogStub {
        id: ProviderId::from(ID),
        auth: auth.clone(),
        models: Vec::new(),
    };
    resolve_provider_auth_for(&store, &empty).await.unwrap();
    {
        let seen = seen.lock().unwrap();
        let probe = seen.last().expect("resolve was consulted");
        assert_eq!(
            probe.id.to_string(),
            ID,
            "the probe is named after the provider"
        );
        assert_eq!(probe.provider.as_str(), ID);
        assert_eq!(probe.base_url, "");
    }

    // A listed catalog: its first row, untouched.
    let row = |id: &str, base_url: &str| Model {
        id: id.into(),
        base_url: base_url.to_string(),
        ..crate::login::auth_probe_model(&ProviderId::from(ID))
    };
    let listed = CatalogStub {
        id: ProviderId::from(ID),
        auth,
        models: vec![
            row("first", "http://catalog-first"),
            row("second", "http://second"),
        ],
    };
    resolve_provider_auth_for(&store, &listed).await.unwrap();
    let seen = seen.lock().unwrap();
    let subject = seen.last().unwrap();
    assert_eq!(
        subject.id.to_string(),
        "first",
        "the first catalog row stands in"
    );
    assert_eq!(subject.base_url, "http://catalog-first");
}
