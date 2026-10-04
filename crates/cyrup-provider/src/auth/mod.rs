//! Authentication: credential store, provider auth strategies, and the resolution precedence
//! engine (arch-01 §3.7 / func-01 §7).

pub mod google_adc;
pub mod helpers;
pub mod oauth;
pub mod resolve;
pub mod store;
pub mod types;

use crate::error::AuthError;
use crate::model::Model;
use std::sync::Arc;

pub use helpers::{auth_credential, env_key, keyless_local};
pub use oauth::{
    AuthEvent, AuthInfoLink, AuthInteraction, AuthPrompt, AuthPromptKind, AuthSelectOption,
    CallbackServer, CallbackServerConfig, OAuthError, Pkce, generate_pkce, oauth_credential,
    poll_oauth_device_code_flow,
};
pub use resolve::{AuthOverrides, resolve_provider_auth};
pub use store::{CredentialStore, InMemoryCredentialStore, ModifyFn};
pub use types::{
    AuthContext, AuthResult, Credential, CredentialInfo, CredentialType, EnvAuthContext, ModelAuth,
    ProviderEnv,
};

/// App-supplied context for a login flow (pi `LoginOptions`, `ai/src/auth/types.ts:206-214`).
///
/// PROV-118 — `02eed88fd` ("add alternative sign in for the openai provider") added this as the
/// optional second parameter of `OAuthAuth.login`. Upstream's doc comment on the one member:
///
/// > Returns the stable ID of this app installation, e.g. sent to OpenAI as its agent host ID.
/// > Called only by login flows that need it, so apps can create the ID on first use and must
/// > return the same ID on every later call.
///
/// Upstream passes the same bag to the api-key `login` as well, but `ApiKeyAuth.login?` is typed
/// with one parameter (`auth/types.ts:175`) so it is unreachable there; only [`OAuthAuth::login`]
/// takes it here.
#[derive(Clone, Default)]
pub struct LoginOptions {
    /// `getDeviceId?: () => string`. A flow that needs an installation id calls it; a flow that
    /// does not never does, which is why the app may create the id lazily.
    pub get_device_id: Option<GetDeviceIdFn>,
}

/// `getDeviceId: () => string` (pi `LoginOptions.getDeviceId`, `ai/src/auth/types.ts:211-213`).
/// Aliased for the same reason [`ModifyFn`] and [`crate::stream::TransformHeadersFn`] are: the
/// inline type trips clippy's `type_complexity` budget wherever it appears.
pub type GetDeviceIdFn = Arc<dyn Fn() -> String + Send + Sync>;

impl LoginOptions {
    /// Options carrying a device-id provider.
    pub fn with_device_id<F>(get_device_id: F) -> Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        Self {
            get_device_id: Some(Arc::new(get_device_id)),
        }
    }

    /// `options?.getDeviceId?.()` — the installation id, if the app supplied a way to get one.
    pub fn device_id(&self) -> Option<String> {
        self.get_device_id.as_ref().map(|f| f())
    }
}

impl std::fmt::Debug for LoginOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginOptions")
            .field("get_device_id", &self.get_device_id.is_some())
            .finish()
    }
}

/// How a provider authenticates (func-01 §4.1: at least one of `api_key | oauth`).
#[derive(Clone, Default)]
pub struct ProviderAuth {
    pub api_key: Option<Arc<dyn ApiKeyAuth>>,
    pub oauth: Option<Arc<dyn OAuthAuth>>,
}

impl ProviderAuth {
    /// API-key-only auth.
    pub fn with_api_key(strategy: Arc<dyn ApiKeyAuth>) -> Self {
        Self {
            api_key: Some(strategy),
            oauth: None,
        }
    }

    /// OAuth-only auth.
    pub fn with_oauth(strategy: Arc<dyn OAuthAuth>) -> Self {
        Self {
            api_key: None,
            oauth: Some(strategy),
        }
    }

    /// `true` if at least one strategy is configured (func-01 §4.1 invariant).
    pub fn is_configured(&self) -> bool {
        self.api_key.is_some() || self.oauth.is_some()
    }
}

/// An API-key resolution strategy (env-var helper, keyless-local, custom). Merges a stored/explicit
/// credential with ambient sources; `None` means "not configured".
#[async_trait::async_trait]
pub trait ApiKeyAuth: Send + Sync {
    fn name(&self) -> &str;

    /// `true` when this strategy implements [`ApiKeyAuth::login`] — the Rust stand-in for
    /// upstream's `typeof strategy.login === "function"`, since `login?` is an OPTIONAL member
    /// (`ai/src/auth/types.ts:166`, *"Absent = ambient-only"*) and a Rust trait default is
    /// indistinguishable from an override at the call site.
    ///
    /// CFG-005: `/login` needs this to decide whether a provider offers an interactive api-key
    /// setup. It previously answered that question by SNIFFING the strategy's display *name*
    /// (`cyrup-config/src/login.rs`'s `api_key_strategy_supports_login`), which cannot see a
    /// multi-secret flow: the four strategies below all need a second (and sometimes third) value
    /// alongside the key, so the single-secret flow the sniffer selected stored a partial
    /// credential and reported success — leaving a provider that looks logged in and cannot
    /// authenticate.
    fn supports_login(&self) -> bool {
        false
    }

    /// Interactive api-key setup — `login?(interaction): Promise<ApiKeyCredential>`
    /// (`ai/src/auth/types.ts:166`). Prompts for whatever the provider needs (key, account id,
    /// project/location, profile) and returns the [`Credential::ApiKey`] to persist.
    ///
    /// The default reports [`oauth::OAuthError::LoginUnsupported`], matching upstream's absent
    /// `login`: an env-var strategy has no interactive setup, only ambient resolution. Guard a call
    /// with [`ApiKeyAuth::supports_login`] to distinguish "declined" from "not offered".
    async fn login(
        &self,
        _interaction: &dyn oauth::AuthInteraction,
    ) -> Result<Credential, oauth::OAuthError> {
        Err(oauth::OAuthError::LoginUnsupported {
            name: self.name().to_string(),
        })
    }

    /// `true` when this strategy implements [`ApiKeyAuth::check`] — the Rust stand-in for
    /// upstream's `if (apiKey.check)` (`ai/src/auth/types.ts:180-186`, consulted at
    /// `ai/src/models.ts:507`), since `check?` is an OPTIONAL member and a Rust trait default is
    /// indistinguishable from an override at the call site.
    ///
    /// The distinction is load-bearing and is NOT the same as `check()` answering `Ok(None)`:
    /// absent means *"Models checks availability by resolving auth"* (fall through to
    /// [`ApiKeyAuth::resolve`]), whereas present-and-`None` means *"this provider is not
    /// configured"* and stops there.
    fn supports_check(&self) -> bool {
        false
    }

    /// Optional side-effect-free availability check —
    /// `check?(input): Promise<AuthCheck | undefined>` (`ai/src/auth/types.ts:180-186`).
    ///
    /// Upstream's doc comment: *"Optional side-effect-free availability check. Use this when
    /// `resolve()` may execute commands or perform other request-time work. Missing means Models
    /// checks availability by resolving auth."* [`crate::collection::Models::check_auth`] consults
    /// it at pi's exact position (`ai/src/models.ts:504-517`): after the stored-OAuth branch and
    /// after the "no api-key strategy" guard, and INSTEAD OF the resolution path — so a strategy
    /// whose `resolve` shells out is asked a cheap question rather than made to do the work.
    ///
    /// `cred` is the stored credential and is `None` unless it is a [`Credential::ApiKey`], mirroring
    /// `credential?.type === "api_key" ? credential : undefined` (`ai/src/models.ts:510`).
    ///
    /// An `Err` PROPAGATES out of `check_auth`/`get_available` exactly as upstream's
    /// `ModelsError("auth", ...)` rethrow does (`ai/src/models.ts:514-516`); it is never folded into
    /// "unconfigured".
    ///
    /// The default is a no-op that is never reached, because [`ApiKeyAuth::supports_check`] gates
    /// the call — no built-in provider implements `check` upstream either, so every shipped strategy
    /// keeps taking the resolution path. The seam exists for the same reason it exists upstream:
    /// a third-party strategy whose `resolve` is expensive needs a cheap availability answer.
    async fn check(
        &self,
        _ctx: &dyn AuthContext,
        _cred: Option<&Credential>,
    ) -> Result<Option<crate::collection::AuthCheck>, AuthError> {
        Ok(None)
    }

    /// Resolve request auth. `cred` is the explicit/stored credential (when present); a `None` `cred`
    /// means the resolver may consult ambient sources (env vars) via `ctx` (func-01 R-01-011/012).
    async fn resolve(
        &self,
        model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError>;
}

/// An OAuth strategy. `refresh` runs UNDER the credential-store lock (func-01 R-01-014/067).
///
/// Ports `OAuthAuth` (`ai/src/auth/types.ts:189-210`).
#[async_trait::async_trait]
pub trait OAuthAuth: Send + Sync {
    fn name(&self) -> &str;

    /// Whether access through this auth method is backed by a provider **subscription** rather
    /// than metered API billing (`isSubscription`, pi v0.84.1 `ai/src/auth/types.ts:210-211`).
    ///
    /// This is NOT "the credential is an OAuth credential". Upstream sets it on exactly five
    /// flows — Anthropic (Claude Pro/Max) `oauth/anthropic.ts:357`, OpenAI (ChatGPT Plus/Pro)
    /// `oauth/openai-codex.ts:517`, GitHub Copilot `oauth/github-copilot.ts:402`, Kimi Code
    /// `oauth/kimi-coding.ts:297` and xAI (Grok/X) `oauth/xai.ts:231` — and deliberately leaves
    /// it unset on the OAuth flows that still bill per token, i.e. OpenRouter
    /// (`oauth/openrouter.ts:301-311`) and Radius (`oauth/radius.ts:357-361`). pi's own test
    /// pins that split: *"identifies only subscription-backed OAuth flows as subscriptions"*,
    /// `ai/test/oauth-auth.test.ts:30-35`, which asserts `toBe(true)` for the five and
    /// `not.toBe(true)` for OpenRouter.
    ///
    /// Consumers must use this and not `isUsingOAuth`: pi v0.84.0's changelog entry for the TUI
    /// reads *"Fixed the footer showing `(sub)` for generic OAuth/OpenID sign-ins without a
    /// known subscription"* (`coding-agent/CHANGELOG.md:155`).
    ///
    /// **Shape.** Upstream's field is `isSubscription?: boolean` and every consumer compares
    /// `=== true` (`coding-agent/src/core/model-runtime.ts:463`), so absent and `false` are
    /// indistinguishable to a reader; a plain `bool` defaulting to `false` is exactly that
    /// contract.
    fn is_subscription(&self) -> bool {
        false
    }

    /// Selector label for the subscription login option, e.g. `"Sign in with SuperGrok or X
    /// Premium"` (`loginLabel`, `ai/src/auth/types.ts:194`). Optional upstream, hence the
    /// default.
    fn login_label(&self) -> Option<&str> {
        None
    }

    /// Interactive login — the flow that *obtains* a credential (`login`,
    /// `ai/src/auth/types.ts:196`). Drives the user through the browser/device dance via
    /// `interaction` and returns the credential to persist
    /// (`store.modify(provider.id, async () => credential)`).
    ///
    /// The default reports [`oauth::OAuthError::LoginUnsupported`]: upstream makes `login`
    /// mandatory, but a Rust default keeps strategies that only *use* a stored credential (and
    /// the tests that fake them) compiling unchanged. Every real flow overrides it.
    ///
    /// The substrate to implement it lives in [`oauth`]: [`oauth::generate_pkce`],
    /// [`oauth::CallbackServer`], [`oauth::poll_oauth_device_code_flow`] and
    /// [`oauth::oauth_credential`].
    ///
    /// `options` is pi's optional second parameter (`login(interaction, options?)`,
    /// `ai/src/auth/types.ts:226`), added by PROV-118's `02eed88fd` so a flow can ask the app for
    /// a stable installation id. It is a required parameter here rather than an `Option` so a flow
    /// that needs one cannot silently fail to receive it; [`LoginOptions::default`] is the
    /// "called without options" case, and [`LoginOptions::device_id`] is `options?.getDeviceId?.()`.
    async fn login(
        &self,
        _interaction: &dyn oauth::AuthInteraction,
        _options: &LoginOptions,
    ) -> Result<Credential, oauth::OAuthError> {
        Err(oauth::OAuthError::LoginUnsupported {
            name: self.name().to_string(),
        })
    }

    /// Network refresh of an expired credential. A failure surfaces as `AuthError::OAuth` and MUST
    /// NOT fall back to an env key (func-01 R-01-013).
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError>;

    /// Side-effect-free derivation of request auth from a valid credential.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError>;
}
