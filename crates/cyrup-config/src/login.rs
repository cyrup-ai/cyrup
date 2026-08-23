//! `/login` and `/logout` — which provider/flow a login request targets, and persisting the
//! credential it yields (arch-07 §3.5; the config-side half of the login command surface).
//!
//! pi has no `pi login` subcommand: login is a **slash command**, `{ name: "login", description:
//! "Configure provider authentication", argumentHint: "<provider>" }`
//! (`packages/coding-agent/src/core/slash-commands.ts:35`), dispatched at
//! `modes/interactive/interactive-mode.ts:2742-2744`. Everything between "the user typed
//! `/login openrouter`" and "a credential is on disk" is ported here; the dialog that renders the
//! URL/device-code/secret prompt is the front-end's, and the flows that produce the credential
//! already live in `cyrup-provider`'s `auth/oauth/`.
//!
//! Ports pi v0.83.0:
//!
//! | this module | pi source |
//! |---|---|
//! | [`AuthType`] | `packages/ai/src/auth/types.ts:111` |
//! | [`AuthCheck`] | `packages/ai/src/auth/types.ts:106-109` |
//! | [`LoginError`] | `packages/ai/src/auth/resolve.ts:15-39` — `ModelsError`, codes `provider`/`auth` |
//! | [`login`] | `packages/ai/src/models.ts:431-444` (`Models.login`) |
//! | [`logout`] | `packages/ai/src/models.ts:446-452` (`Models.logout`) |
//! | [`provider_auth_status`] | `packages/coding-agent/src/core/model-runtime.ts:428-437` |
//! | [`login_provider_options`] | `.../interactive-mode.ts:4857-4887` (`getLoginProviderOptions`) |
//! | [`logout_provider_options`] | `.../interactive-mode.ts:4889-4898` (`getLogoutProviderOptions`) |
//! | [`find_login_provider_options`] | `.../interactive-mode.ts:4900-4911` |
//! | [`resolve_login_command`] | `.../interactive-mode.ts:4913-4935` (`handleLoginCommand`) |
//! | [`start_provider_login`] | `.../interactive-mode.ts:4937-4945` |
//! | [`resolve_auth_type_selector`] | `.../interactive-mode.ts:4947-4970` (`showLoginAuthTypeSelector`) |
//! | [`provider_selector_empty_message`] | `.../interactive-mode.ts:5006-5016` |
//!
//! ## Mechanism divergences (behaviour is the upstream one)
//!
//! * ~~**`ApiKeyAuth` has no `login` member here.**~~ CLOSED by ADR-0010 step 2 (CFG-005).
//!   `cyrup_provider::auth::ApiKeyAuth` now carries `supports_login`/`login`, the faithful stand-in
//!   for upstream's optional `login?` member (`auth/types.ts:165-166`), and every strategy
//!   implements its own: `EnvKeyAuth` the one-secret prompt (`helpers.ts:12-15`), Cloudflare the
//!   two-prompt one (`providers/cloudflare-auth.ts:48-53`), Vertex the four-prompt one
//!   (`providers/google-vertex.ts:15-45`). [`login`] dispatches to it unconditionally, and the
//!   name sniffer that used to answer "does this strategy have a login?" from `name()` is deleted,
//!   as is this module's `env_api_key_login` — that body now lives where upstream puts it, inside
//!   `envApiKeyAuth` itself (`cyrup_provider::auth::env_key`).
//! * **`Provider` has no `name()`.** Upstream's `Provider` interface carries `name` and
//!   `getLoginProviderOptions` reads it off the registry; cyrup's `Provider` trait exposes only
//!   `id()` (the display name lives on the concrete `WireProvider`). The option builders therefore
//!   take an explicit [`ProviderLoginInput`] carrying the name rather than a `&dyn Provider`.
//! * **`ModelRuntime.login` also refreshes the registry** after persisting
//!   (`model-runtime.ts:507`). That is model-catalog work, not credential work, and it stays with
//!   the caller — see this module's registration notes.

use std::collections::HashMap;

use cyrup_core::ProviderId;
use cyrup_provider::auth::ProviderAuth;
use cyrup_provider::auth::oauth::{AuthInteraction, OAuthError};

use crate::auth::{AuthSource, AuthStatus, AuthStore, Credential};

/// `"Sign in with an account"` — the default subscription label (`interactive-mode.ts:4951`).
pub const DEFAULT_SUBSCRIPTION_LABEL: &str = "Sign in with an account";
/// `"Sign in with an API key"` (`interactive-mode.ts:4952`).
pub const API_KEY_LABEL: &str = "Sign in with an API key";
/// `showStatus("No login methods available.")` (`interactive-mode.ts:4965`).
pub const NO_LOGIN_METHODS: &str = "No login methods available.";
/// The `/logout`-with-nothing-stored status (`interactive-mode.ts:5055-5059`), verbatim — pi spells
/// out that `/logout` is narrower than it looks.
pub const NO_STORED_CREDENTIALS: &str = "No stored credentials to remove. /logout only removes credentials saved by /login; environment variables and models.json config are unchanged.";
/// The strategy name `cyrup_provider::auth::keyless_local` reports (`auth/helpers.rs`). Kept as a
/// named constant because it is the one built-in api-key strategy with NO upstream `login`
/// (pi's local servers need no credential), i.e. the one whose `supports_login()` is `false`.
pub const KEYLESS_LOCAL_STRATEGY: &str = "keyless-local";

/// `AuthType` (`ai/src/auth/types.ts:111`). [`AuthType::as_str`] is the wire spelling the
/// "does not support … login" message interpolates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AuthType {
    /// `"api_key"`.
    ApiKey,
    /// `"oauth"`.
    Oauth,
}

impl AuthType {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthType::ApiKey => "api_key",
            AuthType::Oauth => "oauth",
        }
    }

    /// `formatAuthSelectorProviderType` (`components/oauth-selector.ts:22-24`).
    pub fn selector_label(self) -> &'static str {
        match self {
            AuthType::ApiKey => "API key",
            AuthType::Oauth => "subscription",
        }
    }
}

impl std::fmt::Display for AuthType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `AuthCheck` (`ai/src/auth/types.ts:106-109`) — the non-secret "what is already configured" line
/// a selector row shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthCheck {
    pub auth_type: AuthType,
    pub source: Option<String>,
}

/// One provider as the login surface sees it. Upstream reads `provider.id` / `provider.name` /
/// `provider.auth` off the registry (`interactive-mode.ts:4858`); cyrup's `Provider` trait has no
/// `name()`, so the caller supplies it (see the module divergence note).
pub struct ProviderLoginInput {
    pub id: ProviderId,
    /// `provider.name` — the display name (`WireProvider::name()`).
    pub name: String,
    /// `provider.auth` (`cyrup_provider::Provider::provider_auth()`).
    pub auth: ProviderAuth,
    /// `this.session.modelRuntime.getProviderAuthStatus(provider.id)`
    /// (`interactive-mode.ts:4860`) — build it with [`provider_auth_status`].
    pub status: AuthStatus,
    /// `this.session.modelRuntime.isUsingOAuth(provider.id)` (`interactive-mode.ts:4863`,
    /// `model-runtime.ts:368-370`): the resolved credential for this provider is an OAuth one.
    pub using_oauth: bool,
}

/// `AuthSelectorProvider` (`components/oauth-selector.ts:14-20`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginProviderOption {
    pub id: ProviderId,
    pub name: String,
    pub auth_type: AuthType,
    /// `method.name` — `"Anthropic (Claude Pro/Max)"` for OAuth, `"Anthropic API key"` for an
    /// api-key strategy (see [`cyrup_provider::auth::ApiKeyAuth::name`]).
    pub method_name: Option<String>,
    /// `OAuthAuth.loginLabel` (`ai/src/auth/types.ts:194`), OAuth options only.
    pub login_label: Option<String>,
    /// `method.login !== undefined` — `false` means the credential is configured outside cyrup and
    /// the front-end shows the ambient dialog instead (`interactive-mode.ts:4942`).
    pub supports_login: bool,
    pub status: Option<AuthCheck>,
}

/// A login failure. pi throws `ModelsError(code, message)` and every caller surfaces
/// `error.message` only (`auth/resolve.ts:33`), so [`std::fmt::Display`] reproduces those messages
/// exactly while [`LoginError::code`] keeps the `ModelsErrorCode` (`auth/resolve.ts:15`).
#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    /// `throw new ModelsError("provider", \`Unknown provider: ${providerId}\`)` (`models.ts:433`).
    #[error("Unknown provider: {0}")]
    UnknownProvider(String),

    /// `throw new ModelsError("auth", \`${provider.name} does not support ${type} login\`)`
    /// (`models.ts:436`). The name is the **provider's**, not the strategy's.
    #[error("{name} does not support {auth_type} login")]
    Unsupported { name: String, auth_type: AuthType },

    /// The flow itself failed or was cancelled. pi lets the flow's own `Error` propagate out of
    /// `method.login(interaction)` (`models.ts:437`) and the dialog compares its message against
    /// `"Login cancelled"` (`interactive-mode.ts:5319`), so the message must pass through
    /// untouched.
    #[error("{0}")]
    Flow(#[from] OAuthError),

    /// ``throw new ModelsError("auth", `Credential store modify failed for ${providerId}`,
    /// { cause })`` (`models.ts:441`); `withCauseDetail` appends `": <cause>"`
    /// (`auth/resolve.ts:33-39`).
    #[error("Credential store modify failed for {provider}: {message}")]
    StoreModify { provider: String, message: String },

    /// `throw new ModelsError("auth", \`Credential store delete failed for ${providerId}\`,
    /// { cause })` (`models.ts:450`).
    #[error("Credential store delete failed for {provider}: {message}")]
    StoreDelete { provider: String, message: String },
}

impl LoginError {
    /// The `ModelsErrorCode` pi tags the throw with (`auth/resolve.ts:15`).
    pub fn code(&self) -> &'static str {
        match self {
            LoginError::UnknownProvider(_) => "provider",
            LoginError::Flow(_) => "oauth",
            LoginError::Unsupported { .. }
            | LoginError::StoreModify { .. }
            | LoginError::StoreDelete { .. } => "auth",
        }
    }

    /// `errorMsg !== "Login cancelled"` — the one message the login dialogs swallow instead of
    /// reporting (`interactive-mode.ts:5210`, `:5319`).
    pub fn is_cancelled(&self) -> bool {
        self.to_string() == cyrup_provider::auth::oauth::CANCEL_MESSAGE
    }
}

/// Where [`login`] persists the credential it obtained.
///
/// pi's `Models` holds one `CredentialStore` and writes through
/// `this.credentials.modify(providerId, async () => credential)` (`models.ts:438`); in the
/// coding-agent that store is the `auth.json`-backed `AuthStorage` (`model-runtime.ts:136`), which
/// is [`AuthStore`] here. The trait exists so the same [`login`] also drives
/// `cyrup_provider::CredentialStore` (via [`ProviderCredentialSink`]) — the store the composed
/// model registry reads.
///
/// The error is a bare `String` on purpose: pi only ever interpolates the cause's *message* into
/// the `ModelsError` it throws (`withCauseDetail`, `auth/resolve.ts:33-39`).
#[async_trait::async_trait]
pub trait LoginStore: Send + Sync {
    /// `credentials.modify(providerId, async () => credential)` (`models.ts:438`).
    async fn persist(&self, provider: &ProviderId, credential: Credential) -> Result<(), String>;

    /// `credentials.delete(providerId)` (`models.ts:448`).
    async fn remove(&self, provider: &ProviderId) -> Result<(), String>;
}

#[async_trait::async_trait]
impl LoginStore for AuthStore {
    async fn persist(&self, provider: &ProviderId, credential: Credential) -> Result<(), String> {
        self.modify(provider, |_current| async move { Ok(Some(credential)) })
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn remove(&self, provider: &ProviderId) -> Result<(), String> {
        AuthStore::delete(self, provider)
            .await
            .map_err(|e| e.to_string())
    }
}

/// Adapts a `cyrup_provider::CredentialStore` to [`LoginStore`].
///
/// A newtype rather than a blanket impl because coherence cannot rule out [`AuthStore`] gaining a
/// `CredentialStore` impl later, which would make the two overlap.
pub struct ProviderCredentialSink(pub std::sync::Arc<dyn cyrup_provider::CredentialStore>);

#[async_trait::async_trait]
impl LoginStore for ProviderCredentialSink {
    async fn persist(&self, provider: &ProviderId, credential: Credential) -> Result<(), String> {
        let credential = cyrup_provider::Credential::from(credential);
        self.0
            .modify(
                provider,
                Box::new(move |_current| Box::pin(async move { Ok(Some(credential)) })),
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn remove(&self, provider: &ProviderId) -> Result<(), String> {
        self.0.delete(provider).await.map_err(|e| e.to_string())
    }
}

/// The two `Credential` types are the same `auth.json` record — same serde tag, same fields
/// (`cyrup-provider/src/auth/types.rs:14-30` vs `crate::auth::Credential`). The duplication is
/// pre-existing; these conversions are the seam between them.
impl From<Credential> for cyrup_provider::Credential {
    fn from(value: Credential) -> Self {
        match value {
            Credential::ApiKey { key, env } => cyrup_provider::Credential::ApiKey { key, env },
            Credential::Oauth {
                refresh,
                access,
                expires,
                ext,
            } => cyrup_provider::Credential::Oauth {
                refresh,
                access,
                expires,
                ext,
            },
        }
    }
}

/// The inverse of the above — a login flow returns `cyrup_provider::Credential` and the file store
/// stores [`crate::auth::Credential`].
pub fn credential_from_provider(value: cyrup_provider::Credential) -> Credential {
    match value {
        cyrup_provider::Credential::ApiKey { key, env } => Credential::ApiKey { key, env },
        cyrup_provider::Credential::Oauth {
            refresh,
            access,
            expires,
            ext,
        } => Credential::Oauth {
            refresh,
            access,
            expires,
            ext,
        },
    }
}

/// `ModelRuntime.getProviderAuthStatus` (`model-runtime.ts:428-437`) over the file store:
/// runtime `--api-key` → stored credential → environment variable → nothing.
///
/// This is the ONLY status function. `AuthStore::get_auth_status` — which reported
/// `configured: false` for the runtime and environment tiers, the opposite of upstream, on a cite
/// (`auth-storage.ts:354-369`) that resolves past the end of a 271-line file at v0.83.0 — was
/// deleted by CFG-044. At v0.83.0 all three tiers report `configured: true` and only the `source`
/// distinguishes them.
pub fn provider_auth_status(
    store: &AuthStore,
    provider: &ProviderId,
    env: Option<&HashMap<String, String>>,
) -> AuthStatus {
    // `if (this.credentials.hasRuntimeApiKey(providerId)) return { configured: true, source: "runtime" }`.
    if store.runtime_api_key(provider).is_some() {
        return AuthStatus {
            configured: true,
            source: Some(AuthSource::Runtime),
            label: None,
        };
    }
    // `if (this.snapshot.storedProviders.has(providerId)) return { configured: true, source: "stored" }`.
    if matches!(store.list(), Ok(ids) if ids.iter().any(|id| id == provider.as_str())) {
        return AuthStatus {
            configured: true,
            source: Some(AuthSource::Stored),
            label: None,
        };
    }
    // `return check ? { configured: true, source: "environment", label: check.source } : { configured: false }`.
    if let Some(name) =
        crate::env_keys::find_env_keys_in(provider.as_str(), env, store.ambient_tier())
            .and_then(|keys| keys.into_iter().next())
    {
        return AuthStatus {
            configured: true,
            source: Some(AuthSource::Environment),
            label: Some(name),
        };
    }
    AuthStatus {
        configured: false,
        source: None,
        label: None,
    }
}

/// `AuthStatus.source` as the string upstream stores in `AuthCheck.source`
/// (`model-runtime.ts:429-436`).
fn auth_source_label(source: AuthSource) -> &'static str {
    match source {
        AuthSource::Stored => "stored",
        AuthSource::Runtime => "runtime",
        AuthSource::Environment => "environment",
        AuthSource::Fallback => "fallback",
        AuthSource::ModelsJsonKey => "models.json key",
        AuthSource::ModelsJsonCommand => "models.json command",
    }
}

/// `a.name.localeCompare(b.name)` (`interactive-mode.ts:4886`, `:4897`) — Node's default collation,
/// not a byte comparison. Same `feruca` (pure-Rust UCA) configuration proven against Node in
/// `model.rs:130` and `cyrup-tools`' `ls.rs:100`.
fn sort_by_name(options: &mut [LoginProviderOption]) {
    let mut collator = feruca::Collator::new(feruca::Tailoring::default(), false, true);
    options.sort_by(|a, b| collator.collate(a.name.as_str(), b.name.as_str()));
}

/// `getLoginProviderOptions` (`interactive-mode.ts:4857-4887`): every provider that can be logged
/// into, one row per available auth method, optionally narrowed to one method, sorted by display
/// name.
pub fn login_provider_options(
    providers: &[ProviderLoginInput],
    auth_type: Option<AuthType>,
) -> Vec<LoginProviderOption> {
    let mut options: Vec<LoginProviderOption> = Vec::new();
    for provider in providers {
        // `const status = authStatus.configured ? { type: isUsingOAuth(id) ? "oauth" : "api_key",
        //   source: authStatus.label ?? authStatus.source } : undefined` (:4861-4866).
        let status = if provider.status.configured {
            Some(AuthCheck {
                auth_type: if provider.using_oauth {
                    AuthType::Oauth
                } else {
                    AuthType::ApiKey
                },
                source: provider.status.label.clone().or_else(|| {
                    provider
                        .status
                        .source
                        .map(|s| auth_source_label(s).to_string())
                }),
            })
        } else {
            None
        };

        // `if ((!authType || authType === "oauth") && provider.auth.oauth)` (:4867).
        if matches!(auth_type, None | Some(AuthType::Oauth))
            && let Some(oauth) = provider.auth.oauth.as_ref()
        {
            options.push(LoginProviderOption {
                id: provider.id.clone(),
                name: provider.name.clone(),
                auth_type: AuthType::Oauth,
                method_name: Some(oauth.name().to_string()),
                login_label: oauth.login_label().map(str::to_string),
                // Upstream's `OAuthAuth.login` is a required member (`auth/types.ts:196`), so an
                // OAuth option always offers a login.
                supports_login: true,
                status: status.clone(),
            });
        }

        // `if ((!authType || authType === "api_key") && provider.auth.apiKey)` (:4876).
        if matches!(auth_type, None | Some(AuthType::ApiKey))
            && let Some(api_key) = provider.auth.api_key.as_ref()
        {
            options.push(LoginProviderOption {
                id: provider.id.clone(),
                name: provider.name.clone(),
                auth_type: AuthType::ApiKey,
                // `method: provider.auth.apiKey` (:4880) — the option carries the strategy whole,
                // so its label is `method.name` verbatim. No reconstruction: `env_key` now carries
                // upstream's display string (`ai/src/auth/helpers.ts:9`).
                method_name: Some(api_key.name().to_string()),
                login_label: None,
                // `method.login !== undefined` (`interactive-mode.ts:4942`).
                supports_login: api_key.supports_login(),
                status: status.clone(),
            });
        }
    }
    sort_by_name(&mut options);
    options
}

/// `getLogoutProviderOptions` (`interactive-mode.ts:4889-4898`): only providers with a **stored**
/// credential, each carrying `{ type, source: "stored credential" }`.
///
/// `stored` is `listCredentials()` — `(providerId, credential.type)` pairs
/// (`auth-storage.ts:252-254`); read it with [`stored_credentials`]. `known` supplies
/// `getProvider(providerId)?.name ?? providerId` (:4892).
pub fn logout_provider_options(
    stored: &[(ProviderId, AuthType)],
    known: &[ProviderLoginInput],
) -> Vec<LoginProviderOption> {
    let mut options: Vec<LoginProviderOption> = stored
        .iter()
        .map(|(id, auth_type)| LoginProviderOption {
            id: id.clone(),
            name: known
                .iter()
                .find(|p| p.id.as_str() == id.as_str())
                .map(|p| p.name.clone())
                .unwrap_or_else(|| id.as_str().to_string()),
            auth_type: *auth_type,
            method_name: None,
            login_label: None,
            supports_login: false,
            status: Some(AuthCheck {
                auth_type: *auth_type,
                source: Some("stored credential".to_string()),
            }),
        })
        .collect();
    sort_by_name(&mut options);
    options
}

/// `ModelRuntime.listCredentials()` (`model-runtime.ts:424`): the `(provider, type)` pairs the
/// credential store enumerates, in the store's own order.
///
/// Upstream that call is `RuntimeCredentials.list()` (`runtime-credentials.ts:29-36`), i.e.
/// `AuthStorage.list()` **overlaid with the runtime `--api-key` providers** — so a provider whose
/// key came from `--api-key` and never touched `auth.json` still appears. cyrup's [`AuthStore`]
/// fuses both of Pi's layers, and [`AuthStore::list_credentials`] is the composed view; reading
/// the file tier alone dropped the runtime overlay and hid those providers from `/logout`.
///
/// Async only for call-site compatibility — the composed read is synchronous and touches the file
/// exactly once (it used to be one read to list plus one more per provider).
pub async fn stored_credentials(store: &AuthStore) -> Result<Vec<(ProviderId, AuthType)>, String> {
    Ok(store
        .list_credentials()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|info| {
            let auth_type = match info.credential_type {
                cyrup_provider::CredentialType::ApiKey => AuthType::ApiKey,
                cyrup_provider::CredentialType::Oauth => AuthType::Oauth,
            };
            (info.provider, auth_type)
        })
        .collect())
}

/// `findLoginProviderOptions` (`interactive-mode.ts:4900-4911`): case-insensitive match of the
/// `/login <provider>` argument against **either** the provider id or its display name. An
/// argument that is empty after trimming matches nothing.
pub fn find_login_provider_options(
    options: &[LoginProviderOption],
    provider_ref: &str,
) -> Vec<LoginProviderOption> {
    let normalized = provider_ref.trim().to_lowercase();
    if normalized.is_empty() {
        return Vec::new();
    }
    options
        .iter()
        .filter(|option| {
            option.id.as_str().to_lowercase() == normalized
                || option.name.to_lowercase() == normalized
        })
        .cloned()
        .collect()
}

/// What `/login` should do next (`handleLoginCommand`, `interactive-mode.ts:4913-4935`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginCommand {
    /// `showLoginAuthTypeSelector(providerOptions?)` — `options` is `Some` only when the argument
    /// pinned a single provider that offers both methods (:4926-4931).
    AuthTypeSelector {
        options: Option<Vec<LoginProviderOption>>,
    },
    /// `startProviderLogin(providerOptions[0])` (:4922).
    Start(Box<LoginProviderOption>),
    /// `showLoginProviderSelector(undefined, providerRef)` (:4934) — the full picker, seeded with
    /// the unmatched argument as its search text.
    ProviderSelector {
        auth_type: Option<AuthType>,
        initial_search: Option<String>,
    },
}

/// `handleLoginCommand` (`interactive-mode.ts:4913-4935`).
///
/// `provider_ref` is `None` for a bare `/login`. `Some("")` is the same thing — that is what
/// `"/login ".slice(7).trim()` produces and `if (!providerRef)` treats it as absent
/// (`interactive-mode.ts:2743`). A non-empty argument that trims to nothing is *truthy* in JS and
/// therefore falls through to the picker, which this reproduces.
pub fn resolve_login_command(
    provider_ref: Option<&str>,
    options: &[LoginProviderOption],
) -> LoginCommand {
    // `if (!providerRef) { this.showLoginAuthTypeSelector(); return; }` (:4915-4918).
    let Some(provider_ref) = provider_ref.filter(|r| !r.is_empty()) else {
        return LoginCommand::AuthTypeSelector { options: None };
    };

    let matched = find_login_provider_options(options, provider_ref);

    // `if (providerOptions.length === 1) { await this.startProviderLogin(providerOptions[0]!); }` (:4921-4924).
    if let [only] = matched.as_slice() {
        return LoginCommand::Start(Box::new(only.clone()));
    }

    // `if (providerOptions.length > 1) { if (providerIds.size === 1) { showLoginAuthTypeSelector(providerOptions); } }` (:4926-4932).
    if matched.len() > 1 {
        let first = matched.first().map(|o| o.id.clone());
        if matched.iter().all(|o| Some(&o.id) == first.as_ref()) {
            return LoginCommand::AuthTypeSelector {
                options: Some(matched),
            };
        }
    }

    // `this.showLoginProviderSelector(undefined, providerRef);` (:4934).
    LoginCommand::ProviderSelector {
        auth_type: None,
        initial_search: Some(provider_ref.to_string()),
    }
}

/// Which dialog a chosen option opens (`startProviderLogin`, `interactive-mode.ts:4937-4945`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginStep {
    /// `showLoginDialog(id, name)` — run the OAuth flow (:4939).
    Oauth { id: ProviderId, name: String },
    /// `showApiKeyLoginDialog(id, name)` — prompt for the key (:4941).
    ApiKey { id: ProviderId, name: String },
    /// `showAmbientAuthDialog(providerOption)` (:4943): no login to run. `title` is
    /// `` `${name} setup` `` and `message` is
    /// `` `${method?.name ?? "Authentication"} is configured outside pi.` `` (:5160-5163).
    Ambient {
        id: ProviderId,
        name: String,
        title: String,
        message: String,
    },
}

/// `startProviderLogin` (`interactive-mode.ts:4937-4945`).
pub fn start_provider_login(option: &LoginProviderOption) -> LoginStep {
    if option.auth_type == AuthType::Oauth {
        LoginStep::Oauth {
            id: option.id.clone(),
            name: option.name.clone(),
        }
    } else if option.supports_login {
        LoginStep::ApiKey {
            id: option.id.clone(),
            name: option.name.clone(),
        }
    } else {
        LoginStep::Ambient {
            id: option.id.clone(),
            name: option.name.clone(),
            title: format!("{} setup", option.name),
            message: format!(
                "{} is configured outside pi.",
                option.method_name.as_deref().unwrap_or("Authentication")
            ),
        }
    }
}

/// What the auth-method selector resolves to (`showLoginAuthTypeSelector`,
/// `interactive-mode.ts:4947-4970`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthTypeSelector {
    /// `showStatus("No login methods available.")` (:4964-4967).
    Unavailable,
    /// `void this.startProviderLogin(providerOption)` — one provider, one method, so the selector
    /// is skipped entirely (:4969-4975).
    Start(Box<LoginProviderOption>),
    /// Render the choice. Each label is present only when that method is available (:4956-4961).
    Choose {
        title: String,
        subscription_label: Option<String>,
        api_key_label: Option<String>,
    },
}

/// `showLoginAuthTypeSelector` (`interactive-mode.ts:4947-4970`).
///
/// `provider_options` is `None` for a bare `/login` (both methods offered, choosing one opens the
/// provider picker) and `Some` when the argument already pinned a provider.
pub fn resolve_auth_type_selector(
    provider_options: Option<&[LoginProviderOption]>,
) -> AuthTypeSelector {
    // `oauthProvider?.method && "loginLabel" in method ? method.loginLabel : undefined` (:4948-4950).
    let subscription_label = provider_options
        .and_then(|options| options.iter().find(|o| o.auth_type == AuthType::Oauth))
        .and_then(|o| o.login_label.clone())
        .unwrap_or_else(|| DEFAULT_SUBSCRIPTION_LABEL.to_string());

    // `providerOptions ? new Set(map(authType)) : new Set(["oauth", "api_key"])` (:4953-4955).
    let (has_oauth, has_api_key) = match provider_options {
        Some(options) => (
            options.iter().any(|o| o.auth_type == AuthType::Oauth),
            options.iter().any(|o| o.auth_type == AuthType::ApiKey),
        ),
        None => (true, true),
    };
    let count = usize::from(has_oauth) + usize::from(has_api_key);

    // `if (options.length === 0) { showStatus("No login methods available."); return; }` (:4964-4967).
    if count == 0 {
        return AuthTypeSelector::Unavailable;
    }

    // `if (providerOptions && options.length === 1) { startProviderLogin(providerOptions[0]) }` (:4969-4975).
    if count == 1
        && let Some(options) = provider_options
    {
        return match options.first() {
            Some(first) => AuthTypeSelector::Start(Box::new(first.clone())),
            // `const providerOption = providerOptions[0]; if (providerOption) {…}` — an empty
            // array reaches neither arm, and `count == 0` already returned above, so this is
            // unreachable in practice.
            None => AuthTypeSelector::Unavailable,
        };
    }

    AuthTypeSelector::Choose {
        // `providerOptions?.[0] ? \`Select authentication method for ${name}:\` : "Select authentication method:"` (:4977-4979).
        title: match provider_options.and_then(<[LoginProviderOption]>::first) {
            Some(first) => format!("Select authentication method for {}:", first.name),
            None => "Select authentication method:".to_string(),
        },
        subscription_label: has_oauth.then_some(subscription_label),
        api_key_label: has_api_key.then(|| API_KEY_LABEL.to_string()),
    }
}

/// The status shown when the provider picker has nothing to show
/// (`showLoginProviderSelector`, `interactive-mode.ts:5008-5016`).
pub fn provider_selector_empty_message(auth_type: Option<AuthType>) -> &'static str {
    match auth_type {
        Some(AuthType::Oauth) => "No subscription providers available.",
        Some(AuthType::ApiKey) => "No API key providers available.",
        None => "No login providers available.",
    }
}

/// `Models.login` (`ai/src/models.ts:431-444`):
///
/// ```ts
/// const provider = this.providers.get(providerId);
/// if (!provider) throw new ModelsError("provider", `Unknown provider: ${providerId}`);
/// const method = type === "oauth" ? provider.auth.oauth : provider.auth.apiKey;
/// if (!method?.login) throw new ModelsError("auth", `${provider.name} does not support ${type} login`);
/// const credential = await method.login(interaction);
/// try { await this.credentials.modify(providerId, async () => credential); }
/// catch (error) { throw new ModelsError("auth", `Credential store modify failed for ${providerId}`, { cause: error }); }
/// return credential;
/// ```
///
/// The flow runs **before** the store is touched, so a cancelled or failed login leaves whatever
/// was on disk untouched. `ModelRuntime.login` (`model-runtime.ts:505-509`) additionally refreshes
/// the model registry afterwards; that is the caller's step.
pub async fn login(
    store: &dyn LoginStore,
    providers: &[ProviderLoginInput],
    provider_id: &ProviderId,
    auth_type: AuthType,
    interaction: &dyn AuthInteraction,
) -> Result<Credential, LoginError> {
    let Some(provider) = providers
        .iter()
        .find(|p| p.id.as_str() == provider_id.as_str())
    else {
        return Err(LoginError::UnknownProvider(
            provider_id.as_str().to_string(),
        ));
    };

    let credential = match auth_type {
        AuthType::Oauth => {
            let Some(oauth) = provider.auth.oauth.as_ref() else {
                return Err(LoginError::Unsupported {
                    name: provider.name.clone(),
                    auth_type,
                });
            };
            credential_from_provider(oauth.login(interaction).await?)
        }
        AuthType::ApiKey => {
            // `if (!method?.login) throw new ModelsError("auth", …)` (`ai/src/models.ts:433-435`
            // @v0.83.0): no api-key strategy at all, or one that omits `login`. ADR-0010 step 2 —
            // the answer comes from the strategy itself now, not from sniffing its display name.
            let Some(api_key) = provider
                .auth
                .api_key
                .as_ref()
                .filter(|s| s.supports_login())
            else {
                return Err(LoginError::Unsupported {
                    name: provider.name.clone(),
                    auth_type,
                });
            };
            // `await method.login(interaction)` (`ai/src/models.ts:436` @v0.83.0). Every strategy
            // runs its OWN flow: `EnvKeyAuth` the one-secret prompt (`auth/helpers.ts:12-15`), and
            // the multi-prompt ones — Cloudflare Workers AI / AI Gateway, Google Vertex, Amazon
            // Bedrock — the account id / project+location / AWS profile they additionally need.
            // Running the generic one-secret flow for those stored a partial credential and
            // reported success (ADR-0010 §"(3)").
            credential_from_provider(api_key.login(interaction).await?)
        }
    };

    store
        .persist(provider_id, credential.clone())
        .await
        .map_err(|message| LoginError::StoreModify {
            provider: provider_id.as_str().to_string(),
            message,
        })?;
    Ok(credential)
}

/// `Models.logout` (`ai/src/models.ts:446-452`) — delete the stored credential, wrapping a store
/// failure as `Credential store delete failed for ${providerId}`. Environment variables and
/// `models.json` are untouched, which is exactly what [`NO_STORED_CREDENTIALS`] warns about.
pub async fn logout(store: &dyn LoginStore, provider_id: &ProviderId) -> Result<(), LoginError> {
    store
        .remove(provider_id)
        .await
        .map_err(|message| LoginError::StoreDelete {
            provider: provider_id.as_str().to_string(),
            message,
        })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use cyrup_provider::auth::OAuthAuth;
    use cyrup_provider::auth::oauth::ScriptedInteraction;
    use cyrup_provider::{AuthError as ProviderAuthError, CredentialStore, ModelAuth};
    use std::sync::Arc;

    struct StubOauth {
        name: &'static str,
        login_label: Option<&'static str>,
        credential: cyrup_provider::Credential,
    }

    #[async_trait::async_trait]
    impl OAuthAuth for StubOauth {
        fn name(&self) -> &str {
            self.name
        }
        fn login_label(&self) -> Option<&str> {
            self.login_label
        }
        async fn login(
            &self,
            interaction: &dyn AuthInteraction,
        ) -> Result<cyrup_provider::Credential, OAuthError> {
            // Consume one scripted answer so a test can drive a cancel through the flow.
            interaction
                .prompt(cyrup_provider::auth::oauth::AuthPrompt::text(
                    "paste the code",
                ))
                .await?;
            Ok(self.credential.clone())
        }
        async fn refresh(
            &self,
            cred: &cyrup_provider::Credential,
        ) -> Result<cyrup_provider::Credential, ProviderAuthError> {
            Ok(cred.clone())
        }
        async fn to_auth(
            &self,
            _cred: &cyrup_provider::Credential,
        ) -> Result<ModelAuth, ProviderAuthError> {
            Ok(ModelAuth::default())
        }
    }

    fn oauth_credential() -> cyrup_provider::Credential {
        cyrup_provider::Credential::Oauth {
            refresh: "rt".into(),
            access: "at".into(),
            expires: 1_700_000_000_000,
            ext: serde_json::Map::new(),
        }
    }

    fn unconfigured() -> AuthStatus {
        AuthStatus {
            configured: false,
            source: None,
            label: None,
        }
    }

    fn provider(id: &str, name: &str, auth: ProviderAuth) -> ProviderLoginInput {
        ProviderLoginInput {
            id: ProviderId::from(id),
            name: name.to_string(),
            auth,
            status: unconfigured(),
            using_oauth: false,
        }
    }

    /// The REAL `envApiKeyAuth` strategy (`cyrup_provider::auth::env_key`), carrying upstream's
    /// display name the way `providers/openrouter.ts:13` does. CFG-005: these fixtures used to be a
    /// local stub reporting the `"env-key"` sentinel, which meant no test ever exercised
    /// `EnvKeyAuth::supports_login`/`::login` — the two members the sniffer stood in for.
    fn env_key_provider(id: &str, name: &str) -> ProviderLoginInput {
        provider(
            id,
            name,
            ProviderAuth::with_api_key(cyrup_provider::auth::env_key(
                format!("{name} API key"),
                Vec::<String>::new(),
            )),
        )
    }

    fn both_provider(
        id: &str,
        name: &str,
        login_label: Option<&'static str>,
    ) -> ProviderLoginInput {
        provider(
            id,
            name,
            ProviderAuth {
                api_key: Some(cyrup_provider::auth::env_key(
                    format!("{name} API key"),
                    Vec::<String>::new(),
                )),
                oauth: Some(Arc::new(StubOauth {
                    name: "Anthropic (Claude Pro/Max)",
                    login_label,
                    credential: oauth_credential(),
                })),
            },
        )
    }

    // ---------------------------------------------------------------- options

    /// `getLoginProviderOptions` emits one row per available method and sorts by display name
    /// (`interactive-mode.ts:4867-4886`).
    #[test]
    fn options_are_one_per_method_sorted_by_display_name() {
        let providers = vec![
            env_key_provider("openrouter", "OpenRouter"),
            both_provider("anthropic", "Anthropic", None),
        ];
        let options = login_provider_options(&providers, None);
        assert_eq!(
            options
                .iter()
                .map(|o| (o.name.as_str(), o.auth_type))
                .collect::<Vec<_>>(),
            vec![
                ("Anthropic", AuthType::Oauth),
                ("Anthropic", AuthType::ApiKey),
                ("OpenRouter", AuthType::ApiKey),
            ],
            "Anthropic sorts before OpenRouter, and within a provider oauth is pushed first"
        );
        // `method.name`: the OAuth strategy's own name, and the reconstructed api-key label.
        assert_eq!(
            options[0].method_name.as_deref(),
            Some("Anthropic (Claude Pro/Max)")
        );
        assert_eq!(options[1].method_name.as_deref(), Some("Anthropic API key"));
        assert_eq!(
            options[2].method_name.as_deref(),
            Some("OpenRouter API key")
        );
    }

    /// The `authType` argument narrows to one method (`interactive-mode.ts:4867`, `:4876`).
    #[test]
    fn options_filter_by_auth_type() {
        let providers = vec![both_provider("anthropic", "Anthropic", None)];
        assert_eq!(
            login_provider_options(&providers, Some(AuthType::Oauth))
                .iter()
                .map(|o| o.auth_type)
                .collect::<Vec<_>>(),
            vec![AuthType::Oauth]
        );
        assert_eq!(
            login_provider_options(&providers, Some(AuthType::ApiKey))
                .iter()
                .map(|o| o.auth_type)
                .collect::<Vec<_>>(),
            vec![AuthType::ApiKey]
        );
    }

    /// `status` is set only when the provider is configured, and its `type` follows
    /// `isUsingOAuth` — not the row's own auth type (`interactive-mode.ts:4861-4866`).
    #[test]
    fn status_follows_is_using_oauth_not_the_row() {
        let mut p = both_provider("anthropic", "Anthropic", None);
        p.status = AuthStatus {
            configured: true,
            source: Some(AuthSource::Stored),
            label: None,
        };
        p.using_oauth = true;
        let options = login_provider_options(&[p], None);
        for option in &options {
            let status = option.status.as_ref().expect("configured ⇒ status");
            assert_eq!(status.auth_type, AuthType::Oauth);
            assert_eq!(status.source.as_deref(), Some("stored"));
        }

        let mut unconfigured_provider = both_provider("anthropic", "Anthropic", None);
        unconfigured_provider.using_oauth = true;
        assert!(
            login_provider_options(&[unconfigured_provider], None)
                .iter()
                .all(|o| o.status.is_none()),
            "unconfigured ⇒ no status line"
        );
    }

    /// `authStatus.label ?? authStatus.source` (`interactive-mode.ts:4864`).
    #[test]
    fn status_source_prefers_the_label() {
        let mut p = env_key_provider("openrouter", "OpenRouter");
        p.status = AuthStatus {
            configured: true,
            source: Some(AuthSource::Environment),
            label: Some("OPENROUTER_API_KEY".to_string()),
        };
        let options = login_provider_options(&[p], None);
        assert_eq!(
            options[0].status.as_ref().and_then(|s| s.source.as_deref()),
            Some("OPENROUTER_API_KEY")
        );
    }

    /// A `keyless_local` provider offers no login, so the front-end shows the ambient dialog
    /// (`interactive-mode.ts:4940-4944`, `:5163`).
    #[test]
    fn keyless_local_has_no_login_and_lands_on_the_ambient_dialog() {
        let providers = vec![provider(
            "ollama",
            "Ollama",
            ProviderAuth::with_api_key(cyrup_provider::auth::keyless_local()),
        )];
        let options = login_provider_options(&providers, None);
        assert!(!options[0].supports_login);
        assert_eq!(
            start_provider_login(&options[0]),
            LoginStep::Ambient {
                id: ProviderId::from("ollama"),
                name: "Ollama".to_string(),
                title: "Ollama setup".to_string(),
                message: "keyless-local is configured outside pi.".to_string(),
            }
        );
    }

    /// `startProviderLogin`'s two live arms (`interactive-mode.ts:4938-4942`).
    #[test]
    fn start_provider_login_picks_the_dialog() {
        let providers = vec![both_provider("anthropic", "Anthropic", None)];
        let options = login_provider_options(&providers, None);
        assert_eq!(
            start_provider_login(&options[0]),
            LoginStep::Oauth {
                id: ProviderId::from("anthropic"),
                name: "Anthropic".to_string()
            }
        );
        assert_eq!(
            start_provider_login(&options[1]),
            LoginStep::ApiKey {
                id: ProviderId::from("anthropic"),
                name: "Anthropic".to_string()
            }
        );
    }

    // ------------------------------------------------------- target resolution

    /// `findLoginProviderOptions` matches id **or** display name, case-insensitively, and an
    /// argument that trims to nothing matches nothing (`interactive-mode.ts:4901-4910`).
    #[test]
    fn find_matches_id_or_name_case_insensitively() {
        // `zai-coding-cn` / `Z.AI Coding CN` is the real fleet pair whose id and display name
        // share no spelling, so each arm of the `||` is exercised on its own.
        let providers = vec![
            env_key_provider("openrouter", "OpenRouter"),
            env_key_provider("zai-coding-cn", "Z.AI Coding CN"),
            both_provider("anthropic", "Anthropic", None),
        ];
        let options = login_provider_options(&providers, None);

        assert_eq!(find_login_provider_options(&options, "OPENROUTER").len(), 1);
        assert_eq!(find_login_provider_options(&options, "openrouter").len(), 1);
        assert_eq!(
            find_login_provider_options(&options, " OpenRouter ").len(),
            1
        );
        // Id only: the display name does not contain it.
        assert_eq!(
            find_login_provider_options(&options, "zai-coding-cn")
                .iter()
                .map(|o| o.id.as_str().to_string())
                .collect::<Vec<_>>(),
            vec!["zai-coding-cn".to_string()]
        );
        // Display name only: the id does not contain it.
        assert_eq!(
            find_login_provider_options(&options, "z.ai coding cn")
                .iter()
                .map(|o| o.id.as_str().to_string())
                .collect::<Vec<_>>(),
            vec!["zai-coding-cn".to_string()]
        );
        // Matches every method row of the matched provider — here both of Anthropic's.
        assert_eq!(find_login_provider_options(&options, "anthropic").len(), 2);
        assert!(find_login_provider_options(&options, "   ").is_empty());
        assert!(find_login_provider_options(&options, "").is_empty());
        assert!(find_login_provider_options(&options, "nope").is_empty());
        // A *substring* is not a match: upstream compares with `===`, not `includes`.
        assert!(find_login_provider_options(&options, "open").is_empty());
        assert!(find_login_provider_options(&options, "zai").is_empty());
    }

    /// `handleLoginCommand`'s four outcomes (`interactive-mode.ts:4913-4935`).
    #[test]
    fn resolve_login_command_covers_every_branch() {
        let providers = vec![
            env_key_provider("openrouter", "OpenRouter"),
            both_provider("anthropic", "Anthropic", None),
        ];
        let options = login_provider_options(&providers, None);

        // No argument → the auth-type selector with no provider pinned.
        assert_eq!(
            resolve_login_command(None, &options),
            LoginCommand::AuthTypeSelector { options: None }
        );
        // `"/login "` trims to "" and is falsy upstream — same outcome.
        assert_eq!(
            resolve_login_command(Some(""), &options),
            LoginCommand::AuthTypeSelector { options: None }
        );

        // Exactly one match → start it, no selector at all.
        match resolve_login_command(Some("openrouter"), &options) {
            LoginCommand::Start(option) => {
                assert_eq!(option.id.as_str(), "openrouter");
                assert_eq!(option.auth_type, AuthType::ApiKey);
            }
            other => panic!("expected Start, got {other:?}"),
        }

        // Two matches sharing one id → ask which method.
        match resolve_login_command(Some("anthropic"), &options) {
            LoginCommand::AuthTypeSelector {
                options: Some(matched),
            } => {
                assert_eq!(matched.len(), 2);
                assert!(matched.iter().all(|o| o.id.as_str() == "anthropic"));
            }
            other => panic!("expected a pinned AuthTypeSelector, got {other:?}"),
        }

        // No match → the full picker, seeded with the argument.
        assert_eq!(
            resolve_login_command(Some("nope"), &options),
            LoginCommand::ProviderSelector {
                auth_type: None,
                initial_search: Some("nope".to_string()),
            }
        );

        // A whitespace-only argument is truthy in JS, so it reaches the picker rather than the
        // auth-type selector (`interactive-mode.ts:4915` vs `:4934`).
        assert_eq!(
            resolve_login_command(Some("  "), &options),
            LoginCommand::ProviderSelector {
                auth_type: None,
                initial_search: Some("  ".to_string()),
            }
        );
    }

    /// Two providers whose display names collide on the argument → distinct ids → the picker
    /// (`interactive-mode.ts:4926-4934`).
    #[test]
    fn ambiguous_reference_across_providers_opens_the_picker() {
        let providers = vec![
            env_key_provider("moonshotai", "Moonshot"),
            env_key_provider("moonshotai-cn", "Moonshot"),
        ];
        let options = login_provider_options(&providers, None);
        assert_eq!(find_login_provider_options(&options, "moonshot").len(), 2);
        assert_eq!(
            resolve_login_command(Some("moonshot"), &options),
            LoginCommand::ProviderSelector {
                auth_type: None,
                initial_search: Some("moonshot".to_string()),
            }
        );
    }

    /// `showLoginAuthTypeSelector` (`interactive-mode.ts:4947-4979`).
    #[test]
    fn auth_type_selector_labels_and_shortcuts() {
        // Bare `/login`: both methods, generic title, default subscription label.
        assert_eq!(
            resolve_auth_type_selector(None),
            AuthTypeSelector::Choose {
                title: "Select authentication method:".to_string(),
                subscription_label: Some(DEFAULT_SUBSCRIPTION_LABEL.to_string()),
                api_key_label: Some(API_KEY_LABEL.to_string()),
            }
        );

        // Pinned provider offering both: the flow's own `loginLabel` replaces the default.
        let both = login_provider_options(
            &[both_provider(
                "xai",
                "xAI",
                Some("Sign in with SuperGrok or X Premium"),
            )],
            None,
        );
        assert_eq!(
            resolve_auth_type_selector(Some(&both)),
            AuthTypeSelector::Choose {
                title: "Select authentication method for xAI:".to_string(),
                subscription_label: Some("Sign in with SuperGrok or X Premium".to_string()),
                api_key_label: Some(API_KEY_LABEL.to_string()),
            }
        );

        // Pinned provider with one method: skip the selector and start that login.
        let one = login_provider_options(&[env_key_provider("openrouter", "OpenRouter")], None);
        match resolve_auth_type_selector(Some(&one)) {
            AuthTypeSelector::Start(option) => assert_eq!(option.auth_type, AuthType::ApiKey),
            other => panic!("expected Start, got {other:?}"),
        }

        // Nothing at all.
        assert_eq!(
            resolve_auth_type_selector(Some(&[])),
            AuthTypeSelector::Unavailable
        );
    }

    #[test]
    fn empty_picker_messages_match_upstream() {
        assert_eq!(
            provider_selector_empty_message(Some(AuthType::Oauth)),
            "No subscription providers available."
        );
        assert_eq!(
            provider_selector_empty_message(Some(AuthType::ApiKey)),
            "No API key providers available."
        );
        assert_eq!(
            provider_selector_empty_message(None),
            "No login providers available."
        );
    }

    // --------------------------------------------------------------- login/out

    /// The returned `TempDir` guard owns the directory's lifetime — callers MUST bind it
    /// (`let (store, _path, _dir) = temp_store("x");`) or the tree is deleted before use.
    fn temp_store(name: &str) -> (AuthStore, std::path::PathBuf, tempfile::TempDir) {
        let dir = tempfile::Builder::new()
            .prefix(&format!("cyrup-login-{name}-"))
            .tempdir()
            .unwrap();
        let path = dir.path().join("auth.json");
        (AuthStore::at(path.clone()), path, dir)
    }

    /// `Models.login` for OAuth: run the flow, then write what it returned (`models.ts:437-438`).
    #[tokio::test]
    async fn oauth_login_persists_the_flow_credential() {
        let (store, _path, _dir) = temp_store("oauth");
        let providers = vec![both_provider("anthropic", "Anthropic", None)];
        let interaction = ScriptedInteraction::new(vec![Ok("code".to_string())]);

        let credential = login(
            &store,
            &providers,
            &ProviderId::from("anthropic"),
            AuthType::Oauth,
            &interaction,
        )
        .await
        .unwrap();

        assert_eq!(credential, credential_from_provider(oauth_credential()));
        assert_eq!(
            store.read(&ProviderId::from("anthropic")).await.unwrap(),
            Some(credential_from_provider(oauth_credential())),
            "the credential must reach auth.json, not just the return value"
        );
    }

    /// `envApiKeyAuth`'s login: one **secret** prompt reading `Enter ${name}`
    /// (`ai/src/auth/helpers.ts:12-13`).
    #[tokio::test]
    async fn api_key_login_prompts_for_a_secret_and_persists_it() {
        let (store, _path, _dir) = temp_store("apikey");
        let providers = vec![env_key_provider("openrouter", "OpenRouter")];
        let interaction = ScriptedInteraction::new(vec![Ok("sk-or-v1-abc".to_string())]);

        let credential = login(
            &store,
            &providers,
            &ProviderId::from("openrouter"),
            AuthType::ApiKey,
            &interaction,
        )
        .await
        .unwrap();

        let prompts = interaction.prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(
            prompts[0].kind,
            Some(cyrup_provider::auth::AuthPromptKind::Secret)
        );
        assert_eq!(prompts[0].message, "Enter OpenRouter API key");
        assert_eq!(credential.stored_api_key(), Some("sk-or-v1-abc"));
        assert_eq!(
            store
                .read(&ProviderId::from("openrouter"))
                .await
                .unwrap()
                .and_then(|c| c.stored_api_key().map(str::to_string))
                .as_deref(),
            Some("sk-or-v1-abc")
        );
    }

    /// The same [`login`] drives a `cyrup_provider::CredentialStore` (`models.ts:438` writes
    /// through the collection's store, which is what the composed registry reads).
    #[tokio::test]
    async fn login_persists_through_the_provider_credential_store() {
        let inner = Arc::new(cyrup_provider::InMemoryCredentialStore::new());
        let sink = ProviderCredentialSink(inner.clone());
        let providers = vec![env_key_provider("openrouter", "OpenRouter")];
        let interaction = ScriptedInteraction::new(vec![Ok("sk-or-v1-xyz".to_string())]);

        login(
            &sink,
            &providers,
            &ProviderId::from("openrouter"),
            AuthType::ApiKey,
            &interaction,
        )
        .await
        .unwrap();

        let stored = inner
            .read(&ProviderId::from("openrouter"))
            .await
            .unwrap()
            .expect("credential in the provider store");
        match stored {
            cyrup_provider::Credential::ApiKey { key, .. } => {
                assert_eq!(key.as_deref(), Some("sk-or-v1-xyz"));
            }
            other => panic!("expected an api_key credential, got {other:?}"),
        }
    }

    /// `throw new ModelsError("provider", \`Unknown provider: ${providerId}\`)` (`models.ts:433`).
    #[tokio::test]
    async fn unknown_provider_message_and_code() {
        let (store, _path, _dir) = temp_store("unknown");
        let interaction = ScriptedInteraction::new(vec![Ok("x".to_string())]);
        let error = login(
            &store,
            &[],
            &ProviderId::from("nope"),
            AuthType::Oauth,
            &interaction,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "Unknown provider: nope");
        assert_eq!(error.code(), "provider");
    }

    /// `${provider.name} does not support ${type} login` (`models.ts:436`) — the **provider's**
    /// name, and the wire spelling of the type.
    #[tokio::test]
    async fn unsupported_method_message_and_code() {
        let (store, _path, _dir) = temp_store("unsupported");
        let providers = vec![env_key_provider("openrouter", "OpenRouter")];
        let interaction = ScriptedInteraction::new(Vec::new());

        let error = login(
            &store,
            &providers,
            &ProviderId::from("openrouter"),
            AuthType::Oauth,
            &interaction,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "OpenRouter does not support oauth login");
        assert_eq!(error.code(), "auth");

        // An ambient-only api-key strategy is `!method?.login` too.
        let ambient = vec![provider(
            "ollama",
            "Ollama",
            ProviderAuth::with_api_key(cyrup_provider::auth::keyless_local()),
        )];
        let error = login(
            &store,
            &ambient,
            &ProviderId::from("ollama"),
            AuthType::ApiKey,
            &interaction,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "Ollama does not support api_key login");
    }

    /// A cancelled flow surfaces `"Login cancelled"` verbatim and writes nothing
    /// (`models.ts:437` runs before `:438`; `interactive-mode.ts:5319` compares the message).
    #[tokio::test]
    async fn cancelled_login_writes_nothing_and_keeps_the_message() {
        let (store, path, _dir) = temp_store("cancel");
        let providers = vec![env_key_provider("openrouter", "OpenRouter")];
        let interaction = ScriptedInteraction::new(vec![Err(OAuthError::Cancelled)]);

        let error = login(
            &store,
            &providers,
            &ProviderId::from("openrouter"),
            AuthType::ApiKey,
            &interaction,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "Login cancelled");
        assert!(error.is_cancelled());
        assert!(
            !path.exists(),
            "a failed login must not create or touch auth.json"
        );
    }

    /// `Models.logout` deletes the stored credential (`models.ts:447-451`).
    #[tokio::test]
    async fn logout_removes_the_stored_credential() {
        let (store, _path, _dir) = temp_store("logout");
        let providers = vec![env_key_provider("openrouter", "OpenRouter")];
        let interaction = ScriptedInteraction::new(vec![Ok("sk".to_string())]);
        let id = ProviderId::from("openrouter");

        login(&store, &providers, &id, AuthType::ApiKey, &interaction)
            .await
            .unwrap();
        assert!(store.read(&id).await.unwrap().is_some());

        logout(&store, &id).await.unwrap();
        assert!(store.read(&id).await.unwrap().is_none());
    }

    /// A store that always fails, to pin the two wrapper messages (`models.ts:441`, `:450`).
    struct FailingStore;

    #[async_trait::async_trait]
    impl LoginStore for FailingStore {
        async fn persist(
            &self,
            _provider: &ProviderId,
            _credential: Credential,
        ) -> Result<(), String> {
            Err("disk on fire".to_string())
        }
        async fn remove(&self, _provider: &ProviderId) -> Result<(), String> {
            Err("disk on fire".to_string())
        }
    }

    #[tokio::test]
    async fn store_failures_are_wrapped_with_the_cause_appended() {
        let providers = vec![env_key_provider("openrouter", "OpenRouter")];
        let interaction = ScriptedInteraction::new(vec![Ok("sk".to_string())]);
        let id = ProviderId::from("openrouter");

        let error = login(
            &FailingStore,
            &providers,
            &id,
            AuthType::ApiKey,
            &interaction,
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Credential store modify failed for openrouter: disk on fire"
        );
        assert_eq!(error.code(), "auth");

        let error = logout(&FailingStore, &id).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Credential store delete failed for openrouter: disk on fire"
        );
    }

    // ------------------------------------------------------------ logout rows

    /// `getLogoutProviderOptions` lists only stored credentials, tagged
    /// `"stored credential"` (`interactive-mode.ts:4890-4897`).
    #[tokio::test]
    async fn logout_options_come_from_the_store_only() {
        let (store, _path, _dir) = temp_store("logout-rows");
        let providers = vec![
            env_key_provider("openrouter", "OpenRouter"),
            both_provider("anthropic", "Anthropic", None),
        ];
        let interaction = ScriptedInteraction::new(vec![Ok("sk".into()), Ok("code".into())]);

        login(
            &store,
            &providers,
            &ProviderId::from("openrouter"),
            AuthType::ApiKey,
            &interaction,
        )
        .await
        .unwrap();
        login(
            &store,
            &providers,
            &ProviderId::from("anthropic"),
            AuthType::Oauth,
            &interaction,
        )
        .await
        .unwrap();

        let stored = stored_credentials(&store).await.unwrap();
        let rows = logout_provider_options(&stored, &providers);
        assert_eq!(
            rows.iter()
                .map(|r| (r.name.as_str(), r.auth_type))
                .collect::<Vec<_>>(),
            vec![
                ("Anthropic", AuthType::Oauth),
                ("OpenRouter", AuthType::ApiKey),
            ],
            "only stored credentials, sorted by display name, typed from what is on disk"
        );
        assert_eq!(
            rows[0].status.as_ref().and_then(|s| s.source.as_deref()),
            Some("stored credential")
        );

        // An id with no registered provider falls back to the id itself (:4892).
        let orphan = vec![(ProviderId::from("mystery"), AuthType::ApiKey)];
        assert_eq!(
            logout_provider_options(&orphan, &providers)[0].name,
            "mystery"
        );
    }

    /// `/logout` must list a provider whose key came from `--api-key` and never touched
    /// `auth.json`. Upstream `getLogoutProviderOptions` reads `listCredentials()`
    /// (`interactive-mode.ts:4890`) = `RuntimeCredentials.list()`, which overlays the runtime
    /// api-key providers onto `AuthStorage.list()` (`runtime-credentials.ts:29-36`) — so
    /// `pi --provider openrouter --model x --api-key sk-runtime` + `/logout` shows OpenRouter.
    /// cyrup read the FILE tier only, so that row was missing.
    ///
    /// User action: `cyrup --provider <p> --model <m> --api-key <k>` (main.rs installs it on the
    /// session's `AuthStore`, Pi main.ts:764) then the `/logout` slash command / `Logout` selector.
    #[tokio::test]
    async fn logout_options_include_a_runtime_api_key_provider() {
        let (store, _path, _dir) = temp_store("logout-runtime");
        let providers = vec![
            env_key_provider("openrouter", "OpenRouter"),
            both_provider("anthropic", "Anthropic", None),
        ];
        // Nothing on disk at all — the ONLY credential is the runtime `--api-key`.
        assert!(stored_credentials(&store).await.unwrap().is_empty());

        store.set_runtime_api_key(ProviderId::from("openrouter"), "sk-runtime".to_string());

        let stored = stored_credentials(&store).await.unwrap();
        assert_eq!(
            stored,
            vec![(ProviderId::from("openrouter"), AuthType::ApiKey)],
            "the runtime `--api-key` overlay is part of listCredentials() \
             (runtime-credentials.ts:32-34)"
        );
        let rows = logout_provider_options(&stored, &providers);
        assert_eq!(
            rows.iter()
                .map(|r| (r.name.as_str(), r.auth_type))
                .collect::<Vec<_>>(),
            vec![("OpenRouter", AuthType::ApiKey)],
            "`/logout` must offer the --api-key provider"
        );
    }

    /// The overlay REPLACES the stored entry's type, exactly as upstream's `Map.set` does
    /// (`runtime-credentials.ts:32-34`): a provider holding an OAuth credential on disk that is
    /// then given `--api-key` enumerates as `api_key`, and is listed exactly once.
    #[tokio::test]
    async fn runtime_api_key_overrides_the_stored_credential_type_once() {
        let (store, _path, _dir) = temp_store("logout-runtime-override");
        let id = ProviderId::from("anthropic");
        store
            .modify(&id, |_| async {
                Ok(Some(Credential::Oauth {
                    refresh: "r".into(),
                    access: "a".into(),
                    expires: 0,
                    ext: Default::default(),
                }))
            })
            .await
            .unwrap();
        assert_eq!(
            stored_credentials(&store).await.unwrap(),
            vec![(id.clone(), AuthType::Oauth)]
        );

        store.set_runtime_api_key(id.clone(), "sk-runtime".to_string());
        assert_eq!(
            stored_credentials(&store).await.unwrap(),
            vec![(id, AuthType::ApiKey)],
            "one row, retyped to api_key"
        );
    }

    /// `provider_auth_status` reports `configured: true` for all three tiers at v0.83.0
    /// (`model-runtime.ts:429-436`).
    #[tokio::test]
    async fn provider_auth_status_covers_runtime_stored_and_nothing() {
        let (store, _path, _dir) = temp_store("status");
        let id = ProviderId::from("openrouter");

        assert_eq!(
            provider_auth_status(&store, &id, Some(&HashMap::new())),
            unconfigured()
        );

        store
            .modify(&id, |_| async { Ok(Some(Credential::api_key("sk"))) })
            .await
            .unwrap();
        let stored = provider_auth_status(&store, &id, Some(&HashMap::new()));
        assert!(stored.configured);
        assert_eq!(stored.source, Some(AuthSource::Stored));

        // The runtime `--api-key` tier outranks the stored one.
        store.set_runtime_api_key(id.clone(), "sk-runtime".to_string());
        let runtime = provider_auth_status(&store, &id, Some(&HashMap::new()));
        assert!(runtime.configured);
        assert_eq!(runtime.source, Some(AuthSource::Runtime));
    }

    // ------------------------------------------------------------------ misc

    #[test]
    fn auth_type_strings_are_the_wire_ones() {
        assert_eq!(AuthType::ApiKey.as_str(), "api_key");
        assert_eq!(AuthType::Oauth.as_str(), "oauth");
        assert_eq!(AuthType::ApiKey.selector_label(), "API key");
        assert_eq!(AuthType::Oauth.selector_label(), "subscription");
    }

    /// CFG-005 / ADR-0010 step 2 — the "does this strategy have a login?" answer comes from the
    /// STRATEGY (`method.login !== undefined`, `interactive-mode.ts:4942`), not from sniffing its
    /// display name, and the listed label is `method.name` verbatim (`:4880`).
    ///
    /// RED before this pass: `api_key_strategy_supports_login` answered `name != "keyless-local"`,
    /// so a `Vec<Arc<dyn ApiKeyAuth>>` of strategies whose names collide with neither sentinel —
    /// exactly what `keyless_local()` and a bespoke multi-prompt strategy are — could not be
    /// distinguished, and `env_key`'s own label was rebuilt as `"{provider name} API key"`, which
    /// is wrong for `huggingface` ("Hugging Face token", `providers/huggingface.ts:11`) and for
    /// `moonshotai-cn` ("Moonshot AI API key", `providers/moonshotai-cn.ts:11`).
    #[test]
    fn the_login_option_reads_its_label_and_login_support_off_the_strategy() {
        // pi: `envApiKeyAuth("Hugging Face token", ["HF_TOKEN"])` on a provider NAMED "Hugging Face".
        let hf = provider(
            "huggingface",
            "Hugging Face",
            ProviderAuth::with_api_key(cyrup_provider::auth::env_key(
                "Hugging Face token",
                ["HF_TOKEN"],
            )),
        );
        let options = login_provider_options(&[hf], None);
        assert_eq!(
            options[0].method_name.as_deref(),
            Some("Hugging Face token")
        );
        assert!(
            options[0].supports_login,
            "`envApiKeyAuth` always defines `login` (`ai/src/auth/helpers.ts:12-15`)"
        );

        // A strategy that omits `login` — pi's local servers.
        let local = provider(
            "ollama",
            "Ollama",
            ProviderAuth::with_api_key(cyrup_provider::auth::keyless_local()),
        );
        let options = login_provider_options(&[local], None);
        assert!(!options[0].supports_login);
    }

    /// `EnvKeyAuth::login` is upstream's `envApiKeyAuth` body (`ai/src/auth/helpers.ts:12-15`): one
    /// SECRET prompt reading `Enter {name}`, where `{name}` is the strategy's display string — not
    /// a label rebuilt from the provider's name by the caller.
    #[tokio::test]
    async fn env_key_login_prompts_with_the_strategys_own_display_name() {
        use cyrup_provider::auth::oauth::ScriptedInteraction;
        let strategy = cyrup_provider::auth::env_key("Hugging Face token", ["HF_TOKEN"]);
        let interaction = ScriptedInteraction::new(vec![Ok("hf_secret".to_string())]);
        let cred = strategy.login(&interaction).await.expect("login");
        assert_eq!(
            interaction.prompts()[0].message,
            "Enter Hugging Face token",
            "`message: `Enter ${{name}}`` (`ai/src/auth/helpers.ts:13`)"
        );
        assert_eq!(
            interaction.prompts()[0].kind,
            Some(cyrup_provider::auth::AuthPromptKind::Secret)
        );
        assert!(matches!(
            cred,
            cyrup_provider::Credential::ApiKey { ref key, .. } if key.as_deref() == Some("hf_secret")
        ));
    }

    #[test]
    fn credential_conversion_round_trips_both_variants() {
        let oauth = credential_from_provider(oauth_credential());
        assert_eq!(
            credential_from_provider(cyrup_provider::Credential::from(oauth.clone())),
            oauth
        );
        let api_key = Credential::api_key("sk");
        assert_eq!(
            credential_from_provider(cyrup_provider::Credential::from(api_key.clone())),
            api_key
        );
    }
}
