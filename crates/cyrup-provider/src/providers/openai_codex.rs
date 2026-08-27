//! The OpenAI Codex provider (arch-01 §5) — a 1:1 port of pi v0.83.0
//! `packages/ai/src/providers/openai-codex.ts` plus the runtime half of
//! `packages/ai/src/auth/oauth/openai-codex.ts`.
//!
//! This is the ChatGPT Plus/Pro *subscription* endpoint, not the OpenAI Platform API: there is no
//! API-key auth at all (`openai-codex.ts:13-15` wires `auth: { oauth: … }` and nothing else), the
//! base URL is `https://chatgpt.com/backend-api`, and every catalog row speaks the
//! `openai-codex-responses` wire protocol rather than `openai-responses`.
//!
//! The bearer token is a short-lived JWT minted from a stored refresh token at
//! `https://auth.openai.com/oauth/token`. Its payload carries the ChatGPT account id under the
//! namespaced claim `https://api.openai.com/auth` → `chatgpt_account_id`, which upstream extracts
//! and persists on the credential (`auth/oauth/openai-codex.ts:377-390`) because the Codex wire
//! protocol sends it alongside the bearer.
//!
//! # Catalog provenance — read before refreshing
//!
//! The 7 models in `catalog/openai-codex.json` are the verbatim contents of pi
//! `packages/ai/src/providers/openai-codex.models.ts` at commit `b0c2a90e` (2026-07-17), the LAST
//! revision at which pi tracks this catalog's literal data in git. One commit later (`a9f6a315`,
//! "feat(ai): separate generated model data") the data moved to
//! `packages/ai/src/providers/data/openai-codex.json`, which `.gitignore:11` excludes — so at the
//! ported tag `v0.83.0` the catalog is not obtainable from the repository at all, and `b0c2a90e` is
//! the closest knowable snapshot to it. **Since 2026-08-15 that is true of EVERY embedded catalog,
//! not just this one:** all 35 are generated from `b0c2a90e` by
//! `cargo run -p xtask -- gen-catalogs` (PROV-018/PROV-060), so the "four newer files among 31
//! older ones" split this note used to describe is gone, and `catalog_manifest.json` records one
//! revision with a per-provider source map. Do not hand-edit this file — `gen-catalogs --check`
//! fails if you do.
//!
//! # What is not here
//!
//! * **The wire api registration.** Every row's `api` is `openai-codex-responses`, which
//!   `api/mod.rs:129-155` does not register: until `api::openai_codex_responses::factory` is wired
//!   in, a request against a Codex model resolves auth and then fails the registry lookup with a
//!   terminal `StreamEvent::Error` (`wire.rs`, R-01-008/017/018). The catalog and the credential
//!   handling below are complete and tested; the transport is a separate module. Note that module
//!   carries its OWN account-id extractor — pi has two, `extractAccountId`
//!   (`api/openai-codex-responses.ts:1564-1575`) for the request header and
//!   [`openai_codex_account_id`] here for `auth/oauth/openai-codex.ts:377-382`, which stamps the
//!   credential. Both are ported where upstream puts them.
//! * **`openaiCodexOAuth.login`** (`auth/oauth/openai-codex.ts:392-460`) — the login-method picker,
//!   the PKCE browser flow against `https://auth.openai.com/oauth/authorize` with its fixed
//!   `localhost:1455/auth/callback` listener, and the headless device-code alternative — is ported
//!   in [`crate::auth::oauth::openai_codex::OpenAiCodexOAuthFlow`], beside `auth/oauth/anthropic.rs`
//!   and on that module's substrate ([`crate::auth::generate_pkce`],
//!   [`crate::auth::CallbackServer`], [`crate::auth::poll_oauth_device_code_flow`],
//!   [`crate::auth::AuthInteraction`]). [`OpenAiCodexOAuth`] below is the *runtime half* of the same
//!   upstream object — the two [`crate::auth::OAuthAuth`] members a stored credential needs,
//!   `refresh` and `to_auth`, which the flow delegates to — exactly as
//!   [`super::github_copilot::GitHubCopilotOAuth`] is. [`openai_codex_auth`] wires the full flow so
//!   `/login` reaches it (PROV-029).

use crate::api::{ApiRegistry, builtin_registry};
use crate::auth::oauth::now_ms;
use crate::auth::types::{AuthContext, EnvAuthContext, ModelAuth};
use crate::auth::{
    Credential, CredentialStore, InMemoryCredentialStore, OAuthAuth, ProviderAuth,
};
use crate::error::AuthError;
use crate::model::Model;
use crate::wire::WireProvider;
use cyrup_core::ProviderId;
use serde_json::{Map, Value};
use std::sync::Arc;

/// The provider id (pi `openai-codex.ts:10`).
pub const OPENAI_CODEX_PROVIDER_ID: &str = "openai-codex";

/// The ChatGPT backend base URL (pi `openai-codex.ts:12`).
pub const OPENAI_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";

/// The wire-protocol id every catalog row declares (pi `Provider<"openai-codex-responses">`,
/// `openai-codex.ts:7`). Not in [`crate::known_api`] because no impl is registered for it yet; see
/// the module note.
pub const OPENAI_CODEX_RESPONSES_API: &str = "openai-codex-responses";

/// The strategy label shown in the login picker (pi `openaiCodexOAuth.name`,
/// `auth/oauth/openai-codex.ts:393`, and the `lazyOAuth({ name })` at `openai-codex.ts:14`).
pub const OPENAI_CODEX_OAUTH_NAME: &str = "OpenAI (ChatGPT Plus/Pro)";

/// The public OAuth client id (pi `CLIENT_ID`, `auth/oauth/openai-codex.ts:26`).
pub const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// The OAuth issuer (pi `AUTH_BASE_URL`, `auth/oauth/openai-codex.ts:27`).
pub const OPENAI_CODEX_AUTH_BASE_URL: &str = "https://auth.openai.com";

/// The token endpoint (pi `TOKEN_URL`, `auth/oauth/openai-codex.ts:29`).
pub const OPENAI_CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

/// The namespaced JWT claim holding the ChatGPT account id (pi `JWT_CLAIM_PATH`,
/// `auth/oauth/openai-codex.ts:38`).
pub const OPENAI_CODEX_JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

/// The claim inside [`OPENAI_CODEX_JWT_CLAIM_PATH`] (pi `auth?.chatgpt_account_id`,
/// `auth/oauth/openai-codex.ts:381`).
const CHATGPT_ACCOUNT_ID_CLAIM: &str = "chatgpt_account_id";

/// The `auth.json` key the account id is persisted under (pi `OAuthCredential.accountId`,
/// `auth/oauth/openai-codex.ts:392`); flattened into [`Credential::Oauth`]'s `ext`.
pub const EXT_ACCOUNT_ID: &str = "accountId";

/// The verbatim catalog extracted from pi's generated `openai-codex.models.ts` (see the
/// module-level provenance note).
const OPENAI_CODEX_CATALOG_JSON: &str = include_str!("catalog/openai-codex.json");

/// A failure raised by the Codex OAuth runtime. pi throws bare `Error(message)` strings
/// (`auth/oauth/openai-codex.ts:126/135/172/386`); this carries the SAME message text so the
/// operator-visible string is unchanged, wrapped as [`AuthError::OAuth`]'s `source` because cyrup's
/// introspection paths are typed rather than throwing (`error.rs:1-8`).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct OpenAiCodexAuthError(pub String);

impl OpenAiCodexAuthError {
    fn new(msg: impl Into<String>) -> Self {
        OpenAiCodexAuthError(msg.into())
    }
}

/// The full Codex catalog (1:1 with pi `OPENAI_CODEX_MODELS`). A parse failure yields an empty
/// catalog (surfaced loudly by the count test) rather than a panic (NO-PANIC policy).
pub fn openai_codex_models() -> Vec<Model> {
    serde_json::from_str(OPENAI_CODEX_CATALOG_JSON).unwrap_or_default()
}

/// The Codex [`ProviderAuth`] (pi `openai-codex.ts:13-15`): OAuth **only** — there is no env API
/// key for the ChatGPT subscription endpoint.
///
/// PROV-029: the strategy is the FULL flow
/// ([`crate::auth::oauth::openai_codex::OpenAiCodexOAuthFlow`]) — the login-method picker, the PKCE
/// browser flow and the device-code alternative, plus `refresh`/`to_auth` — not [`OpenAiCodexOAuth`]
/// alone, which left `login` on the [`OAuthAuth`] trait default so `/login openai-codex` reported
/// `LoginUnsupported` against a fully ported flow. See
/// [`super::github_copilot::github_copilot_auth`] for the `lazyOAuth` `[CYRUP-DELTA]` note that
/// applies identically here (`openai-codex.ts:13`).
pub fn openai_codex_auth() -> ProviderAuth {
    ProviderAuth::with_oauth(Arc::new(
        crate::auth::oauth::openai_codex::OpenAiCodexOAuthFlow::new(),
    ))
}

/// Construct the Codex provider over the given credential store + shared api registry.
///
/// The registry must provide the `openai-codex-responses` impl. **`crate::api::register_builtins`
/// registers it** (`api/mod.rs`, `crate::known_api::OPENAI_CODEX_RESPONSES` →
/// `openai_codex_responses::factory`), so any provider built over
/// [`crate::api::builtin_registry`] streams. PROV-041: this doc previously claimed "none is
/// registered today … cannot yet stream", which was false at the tree it was written against and
/// is the kind of citation `CLAUDE.md` makes the provenance record — see
/// `prov041_openai_codex_responses_is_registered`.
pub fn openai_codex_provider_with(
    store: Arc<dyn CredentialStore>,
    registry: Arc<ApiRegistry>,
) -> WireProvider {
    WireProvider::new(
        OPENAI_CODEX_PROVIDER_ID,
        "OpenAI Codex",
        openai_codex_models(),
        openai_codex_auth(),
        store,
        registry,
    )
}

/// Convenience constructor: an in-memory credential store + the built-in api registry.
pub fn openai_codex_provider() -> WireProvider {
    openai_codex_provider_with(
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
    )
}

// ---------------------------------------------------------------------------------------------
// JWT account-id extraction (pi `auth/oauth/openai-codex.ts:100-112` + `:377-390`)
// ---------------------------------------------------------------------------------------------

/// One base64 character → its 6-bit value, or `None` for anything outside the STANDARD alphabet.
///
/// `-` and `_` (the base64**url** alphabet) are deliberately absent: see [`atob`].
fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The HTML `atob` algorithm, which pi calls on the JWT payload (`auth/oauth/openai-codex.ts:105`).
///
/// `[CYRUP-DELTA]` pi uses the ambient `atob`; Rust has none, and the `base64` crate's engines do
/// not expose `atob`'s exact acceptance set, so the ~20 lines are written out. The semantics are
/// the spec's, and they are what makes this decode *strict*:
///
/// * ASCII whitespace is stripped first.
/// * Up to two trailing `=` are removed, but only when the length is already a multiple of four.
/// * A remaining length of `n % 4 == 1` is an error (there is no such base64 string).
/// * Any character outside the **STANDARD** alphabet is an error — including `-` and `_`. A JWT
///   payload is base64**url**-encoded, so a payload whose bytes happen to encode to index 62 or 63
///   makes upstream's `atob` throw, `decodeJwt` return `null`, and the login/refresh fail with
///   "Failed to extract accountId from token". That is upstream's behaviour, reproduced here rather
///   than quietly repaired; ASCII-JSON payloads reach 62/63 only via `>`, `?`, `~` or DEL at an
///   offset `≡ 2 (mod 3)`, which is why it is rare enough to have survived upstream.
///
/// Errors are `None` rather than an exception; every caller is inside pi's `try { … } catch {
/// return null }` (`:104-111`).
fn atob(input: &str) -> Option<Vec<u8>> {
    // `atob` strips ASCII whitespace (space, tab, LF, FF, CR) before decoding.
    let stripped: Vec<u8> = input
        .bytes()
        .filter(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r'))
        .collect();

    let mut data: &[u8] = &stripped;
    if data.len().is_multiple_of(4) {
        for _ in 0..2 {
            match data.last() {
                Some(b'=') => data = data.get(..data.len().saturating_sub(1))?,
                _ => break,
            }
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }

    let mut out: Vec<u8> = Vec::with_capacity(data.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for byte in data {
        acc = (acc << 6) | u32::from(base64_value(*byte)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// pi `decodeJwt` (`auth/oauth/openai-codex.ts:100-112`): exactly three dot-separated parts, the
/// middle one `atob`-decoded and `JSON.parse`d. Any failure is `null`.
///
/// `[CYRUP-DELTA]` `atob` yields a JS "binary string" (one char per byte, latin-1) which
/// `JSON.parse` then reads. The same mapping is applied here — each byte becomes the `char` of that
/// code point — so an ASCII payload (every real one) is byte-identical, and a non-ASCII UTF-8
/// payload is mangled identically to upstream instead of being repaired into valid UTF-8.
fn decode_jwt(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    // `parts.length !== 3` (`:103`): exactly three, no more and no fewer.
    let (_header, payload, _signature) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let decoded = atob(payload)?;
    let latin1: String = decoded.into_iter().map(char::from).collect();
    serde_json::from_str(&latin1).ok()
}

/// pi `getAccountId` (`auth/oauth/openai-codex.ts:377-382`): the `chatgpt_account_id` claim nested
/// under the namespaced `https://api.openai.com/auth` key. A non-string or empty value is `null`.
pub fn openai_codex_account_id(access_token: &str) -> Option<String> {
    let payload = decode_jwt(access_token)?;
    let account_id = payload
        .as_object()?
        .get(OPENAI_CODEX_JWT_CLAIM_PATH)?
        .as_object()?
        .get(CHATGPT_ACCOUNT_ID_CLAIM)?
        .as_str()?;
    if account_id.is_empty() {
        return None; // `accountId.length > 0` (`:381`)
    }
    Some(account_id.to_string())
}

// ---------------------------------------------------------------------------------------------
// The OAuth strategy (pi `auth/oauth/openai-codex.ts:392-460`, minus `login`)
// ---------------------------------------------------------------------------------------------

/// The **runtime half** of OpenAI Codex's OAuth strategy: exchange the stored refresh token for a
/// fresh ChatGPT JWT and use that JWT as the bearer (pi `openaiCodexOAuth`,
/// `auth/oauth/openai-codex.ts:392-460`).
///
/// **Not the wired strategy.** [`openai_codex_auth`] carries
/// [`crate::auth::oauth::openai_codex::OpenAiCodexOAuthFlow`], which owns the full upstream object
/// — the login-method picker, the PKCE browser flow and the device-code alternative, plus
/// `refresh`/`to_auth`. This type is retained because `OpenAiCodexOAuthFlow` **delegates**
/// `refresh` and `to_auth` to it (it holds one as its `runtime` field,
/// `auth/oauth/openai_codex.rs:524`), so those two methods have exactly one implementation.
/// Nothing reaches this type's own [`OAuthAuth`] impl from `/login`; before PROV-029 this WAS the
/// wired strategy, and because it leaves `login` on the trait default, `/login openai-codex`
/// reported `LoginUnsupported` against a fully ported flow.
///
/// `[CYRUP-DELTA]` pi wraps this in `lazyOAuth` (`auth/helpers.ts:38-56`) so the Node-only login
/// code (`node:http` callback server, `node:crypto` state) stays out of browser bundles. Rust links
/// statically and has no bundle to split, so the strategy is constructed directly; the observable
/// behaviour of `refresh`/`to_auth` is unchanged. `login` is not implemented here — see the module
/// note.
pub struct OpenAiCodexOAuth {
    /// Ambient context for HTTP(S)_PROXY / NO_PROXY resolution on the token call, so it honours the
    /// same proxy policy as provider traffic (`stream/sse.rs:181-192`).
    auth_ctx: Arc<dyn AuthContext>,
    /// Test-only redirection of the token endpoint to a loopback listener; always `None` in
    /// production.
    token_url_override: Option<String>,
}

impl Default for OpenAiCodexOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiCodexOAuth {
    pub fn new() -> Self {
        Self {
            auth_ctx: Arc::new(EnvAuthContext),
            token_url_override: None,
        }
    }

    /// Override the ambient auth context (for tests / custom env sources), mirroring
    /// [`WireProvider::with_auth_context`].
    #[must_use]
    pub fn with_auth_context(mut self, ctx: Arc<dyn AuthContext>) -> Self {
        self.auth_ctx = ctx;
        self
    }

    /// Point the token endpoint at a loopback listener. Test-only seam (R-06 "never hit the network
    /// in a test"); it does not exist in release builds.
    #[cfg(test)]
    fn with_token_url(mut self, url: &str) -> Self {
        self.token_url_override = Some(url.to_string());
        self
    }

    fn token_url(&self) -> &str {
        match &self.token_url_override {
            Some(url) => url.as_str(),
            None => OPENAI_CODEX_TOKEN_URL,
        }
    }

    /// pi `refreshAccessToken` (`auth/oauth/openai-codex.ts:160-179`): a form-encoded
    /// `grant_type=refresh_token` POST, then [`Self::read_token_response`].
    ///
    /// A transport failure is `"OpenAI Codex token refresh error: <message>"` (`:176`) — a distinct
    /// string from the HTTP-status failure below, and the only place upstream wraps the cause.
    async fn refresh_access_token(
        &self,
        refresh_token: &str,
    ) -> Result<(String, String, i64), OpenAiCodexAuthError> {
        let url = self.token_url();
        // `new URLSearchParams({ grant_type, refresh_token, client_id })` (`:168-172`) — the field
        // order upstream serializes.
        let body = crate::auth::oauth::encode_query([
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", OPENAI_CODEX_CLIENT_ID),
        ]);

        let client =
            crate::stream::sse::build_client_for_target(url, self.auth_ctx.as_ref(), None, None)
                .await
                .map_err(|e| {
                    OpenAiCodexAuthError::new(format!("OpenAI Codex token refresh error: {e}"))
                })?;
        let response = client
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| {
                OpenAiCodexAuthError::new(format!("OpenAI Codex token refresh error: {e}"))
            })?;

        Self::read_token_response(response, "refresh").await
    }

    /// pi `readTokenResponse` (`auth/oauth/openai-codex.ts:122-146`). Returns
    /// `(access, refresh, expires)` with `expires` already stamped as `Date.now() + expires_in *
    /// 1000` (`:144`) in Unix **milliseconds**, the `auth.json` contract
    /// ([`crate::auth::Credential`]).
    ///
    /// Both failure strings are upstream's, verbatim, including the `operation` word that
    /// distinguishes the `exchange` (login) call from the `refresh` one.
    async fn read_token_response(
        response: reqwest::Response,
        operation: &str,
    ) -> Result<(String, String, i64), OpenAiCodexAuthError> {
        let status = response.status();
        if !status.is_success() {
            // `${text || response.statusText}` (`:125`): an empty body falls back to the reason
            // phrase. reqwest exposes the canonical reason rather than the server's literal one;
            // they agree for every standard code.
            let text = response.text().await.unwrap_or_default();
            let detail = if text.is_empty() {
                status.canonical_reason().unwrap_or("").to_string()
            } else {
                text
            };
            return Err(OpenAiCodexAuthError::new(format!(
                "OpenAI Codex token {operation} failed ({}): {detail}",
                status.as_u16()
            )));
        }

        // `await response.json()` — a non-JSON body throws out of the flow upstream; here the
        // parse failure is reported with the same "missing fields" shape, since from the caller's
        // side both mean "the token endpoint did not answer with a token".
        let text = response.text().await.unwrap_or_default();
        let raw: Value = serde_json::from_str(text.trim()).unwrap_or(Value::Null);

        let obj = raw.as_object();
        // `!json?.access_token || !json.refresh_token || typeof json.expires_in !== "number"`
        // (`:136`). JS truthiness makes `""` missing.
        let access = obj
            .and_then(|o| o.get("access_token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let refresh = obj
            .and_then(|o| o.get("refresh_token"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let expires_in = obj
            .and_then(|o| o.get("expires_in"))
            .and_then(Value::as_f64);

        match (access, refresh, expires_in) {
            (Some(access), Some(refresh), Some(expires_in)) => Ok((
                access.to_string(),
                refresh.to_string(),
                now_ms() + (expires_in * 1000.0) as i64,
            )),
            _ => Err(OpenAiCodexAuthError::new(format!(
                "OpenAI Codex token {operation} response missing fields: {}",
                serde_json::to_string(&raw).unwrap_or_else(|_| "null".to_string())
            ))),
        }
    }
}

/// pi `credentialsFromToken` (`auth/oauth/openai-codex.ts:384-397`): stamp the account id onto the
/// credential, or fail with the exact upstream string.
fn credentials_from_token(
    access: String,
    refresh: String,
    expires: i64,
) -> Result<Credential, OpenAiCodexAuthError> {
    let account_id = openai_codex_account_id(&access)
        .ok_or_else(|| OpenAiCodexAuthError::new("Failed to extract accountId from token"))?;
    let mut ext = Map::new();
    ext.insert(EXT_ACCOUNT_ID.to_string(), Value::String(account_id));
    Ok(Credential::Oauth {
        refresh,
        access,
        expires,
        ext,
    })
}

#[async_trait::async_trait]
impl OAuthAuth for OpenAiCodexOAuth {
    /// pi `openaiCodexOAuth.name` (`auth/oauth/openai-codex.ts:393`).
    fn name(&self) -> &str {
        OPENAI_CODEX_OAUTH_NAME
    }

    /// `isSubscription: true` — set on both the flow (pi v0.84.1
    /// `auth/oauth/openai-codex.ts:517`) and the provider's `lazyOAuth` wrapper
    /// (`providers/openai-codex.ts:15`).
    ///
    /// The strategy [`openai_codex_auth`] actually carries is
    /// [`crate::auth::oauth::openai_codex::OpenAiCodexOAuthFlow`], whose own `is_subscription`
    /// (`auth/oauth/openai_codex.rs:1040`) is the answer `/login` observes; this one must agree
    /// with it because both stand for the same upstream object. Changing one without the other is
    /// the bug this doc used to invite by claiming *this* impl was the wired one.
    fn is_subscription(&self) -> bool {
        true
    }

    /// pi `refresh: (credential) => refreshOpenAICodexToken(credential.refresh)`
    /// (`auth/oauth/openai-codex.ts:456`) → `credentialsFromToken(await
    /// refreshAccessToken(refreshToken))` (`:399-401`).
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        let provider = ProviderId::from(OPENAI_CODEX_PROVIDER_ID);
        let refresh_token = match cred {
            Credential::Oauth { refresh, .. } => refresh.clone(),
            // Unrepresentable upstream: pi types `refresh(credential: OAuthCredential)`, so an
            // api-key credential can never reach it. cyrup's store is one enum, so the arm exists.
            Credential::ApiKey { .. } => {
                return Err(AuthError::oauth(
                    provider,
                    OpenAiCodexAuthError::new(
                        "OpenAI Codex refresh requires an OAuth credential",
                    ),
                ));
            }
        };

        let (access, refresh, expires) = self
            .refresh_access_token(&refresh_token)
            .await
            .map_err(|e| AuthError::oauth(provider.clone(), e))?;

        credentials_from_token(access, refresh, expires)
            .map_err(|e| AuthError::oauth(provider, e))
    }

    /// pi `toAuth` (`auth/oauth/openai-codex.ts:458-460`): `{ apiKey: credential.access }` — the
    /// ChatGPT JWT is the bearer, and nothing else is derived here. The account id rides on the
    /// credential's `ext` for the wire impl to read; the base URL is the catalog's.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        match cred {
            Credential::Oauth { access, .. } => Ok(ModelAuth {
                api_key: Some(access.clone()),
                headers: None,
                base_url: None,
            }),
            Credential::ApiKey { .. } => Err(AuthError::oauth(
                ProviderId::from(OPENAI_CODEX_PROVIDER_ID),
                OpenAiCodexAuthError::new("OpenAI Codex toAuth requires an OAuth credential"),
            )),
        }
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
    use base64::Engine as _;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// PROV-041 — the doc above [`openai_codex_provider_with`] used to assert that no
    /// `openai-codex-responses` impl "is registered today", which made a reader conclude the Codex
    /// provider could resolve auth and never stream. This turns the corrected claim into a
    /// property, so the doc cannot go stale in that direction again.
    ///
    /// **Red before the fix:** the doc comment, not the code, was the defect — this test passed
    /// against the false comment. It is the guard the corrected comment now cites, and it goes red
    /// the moment the registration is dropped (which is the state the old comment described).
    #[test]
    fn prov041_openai_codex_responses_is_registered() {
        let reg = crate::api::builtin_registry();
        let api = cyrup_core::ApiId::from(crate::known_api::OPENAI_CODEX_RESPONSES);
        assert!(
            reg.contains(&api),
            "openai-codex-responses is not registered — the doc on openai_codex_provider_with \
             claims it is (PROV-041)"
        );
        let imp = reg.get(&api).expect("registered => constructible");
        assert_eq!(imp.api(), &api);
    }

    /// An [`AuthContext`] over a fixed map — also how the loopback tests keep proxy resolution
    /// deterministic (an ambient `HTTPS_PROXY` must not reroute a loopback request).
    struct MapEnv(BTreeMap<String, String>);
    #[async_trait::async_trait]
    impl AuthContext for MapEnv {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }

    fn empty_env() -> Arc<dyn AuthContext> {
        Arc::new(MapEnv(BTreeMap::new()))
    }

    // ------------------------------------------------------------------ catalog

    /// pi `OPENAI_CODEX_MODELS` at `b0c2a90e`: 7 models, all on the `openai-codex-responses` wire
    /// api and the ChatGPT backend base URL.
    #[test]
    fn catalog_parses_verbatim_with_expected_count() {
        let models = openai_codex_models();
        assert_eq!(models.len(), 7);
        assert!(
            models
                .iter()
                .all(|m| m.provider.as_str() == OPENAI_CODEX_PROVIDER_ID)
        );
        assert!(
            models
                .iter()
                .all(|m| m.api.as_str() == OPENAI_CODEX_RESPONSES_API)
        );
        assert!(models.iter().all(|m| m.base_url == OPENAI_CODEX_BASE_URL));
        assert!(models.iter().all(|m| m.reasoning));
        assert!(models.iter().all(|m| m.max_tokens == 128_000));

        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "gpt-5.3-codex-spark",
                "gpt-5.4",
                "gpt-5.4-mini",
                "gpt-5.5",
                "gpt-5.6-luna",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
            ]
        );
    }

    /// Two defects in the Codex GPT-5.6 rows, which upstream spells as three hardcoded literals
    /// (v0.84.1 `ai/scripts/generate-models.ts:2594-2622`), not models.dev data:
    ///
    /// 1. VERSION LAG (v0.83.0 → v0.84.1): luna/terra took the 2026-07-30 price cut via
    ///    `withOpenAiLongContextPricing(OPENAI_GPT_56_STANDARD_COSTS[...])` (`:2597`, `:2621`).
    ///    v0.83.0 spelled them inline as `{1, 6, 0.1, 1.25}` / `{2.5, 15, 0.25, 3.125}`
    ///    (`v0.83.0 …` — the `diff` of the `codexModels` literal is exactly those two lines).
    /// 2. PORT BUG (wrong at v0.83.0 too): all three rows carry `contextWindow: CODEX_GPT_56_CONTEXT`,
    ///    which is `272000` at BOTH tags (v0.84.1 `…:2541`, v0.83.0 `…:2352`). cyrup had `372000`
    ///    — a transposed digit that inflated the window by 100k and would let compaction defer past
    ///    the real limit.
    ///
    /// **Both values are now PINNED by the catalog generator, and this test is what they are pinned
    /// against** (`xtask/src/main.rs` `DELTAS`). `openai-codex.json` is regenerated from
    /// `b0c2a90e`, which is 13 days older than the ported tag and still carries `372000` and the
    /// pre-cut prices, so a bare regeneration would revert both. The codex rows are **hardcoded in
    /// `ai/scripts/generate-models.ts`** rather than fetched from models.dev, and that script IS in
    /// git at v0.83.0 — so for these rows the ported tag is directly readable and beats `b0c2a90e`.
    /// This also **REFUTES PROV-059(d)**, which reads the same `372000` off `b0c2a90e` and files
    /// cyrup's `272000` as a 100k understatement: v0.83.0's comment at `…:2349` says in words
    /// "GPT-5.6 follows Codex's 272k catalog limit (formerly 372k)".
    #[test]
    fn the_gpt_5_6_codex_rows_match_the_upstream_literals() {
        let models = openai_codex_models();
        let find = |id: &str| {
            models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("{id} missing"))
                .clone()
        };

        for id in ["gpt-5.6-luna", "gpt-5.6-sol", "gpt-5.6-terra"] {
            assert_eq!(find(id).context_window, 272_000, "{id} contextWindow");
        }

        let luna = find("gpt-5.6-luna");
        assert_eq!(
            (
                luna.cost.input,
                luna.cost.output,
                luna.cost.cache_read,
                luna.cost.cache_write
            ),
            (0.2, 1.2, 0.02, 0.25)
        );
        let t = &luna.cost.tiers.as_ref().expect("luna tiers")[0];
        assert_eq!(t.input_tokens_above, 272_000);
        assert_eq!((t.input, t.output, t.cache_read, t.cache_write), (0.4, 1.8, 0.04, 0.5));

        let terra = find("gpt-5.6-terra");
        assert_eq!(
            (
                terra.cost.input,
                terra.cost.output,
                terra.cost.cache_read,
                terra.cost.cache_write
            ),
            (2.0, 12.0, 0.2, 2.5)
        );
        let t = &terra.cost.tiers.as_ref().expect("terra tiers")[0];
        assert_eq!((t.input, t.output, t.cache_read, t.cache_write), (4.0, 18.0, 0.4, 5.0));

        // MIRROR: Sol has no entry in `OPENAI_GPT_56_STANDARD_COSTS`; its literal is the unchanged
        // inline `{5, 30, 0.5, 6.25}` (v0.84.1 `…:2609`), so only its contextWindow moved.
        let sol = find("gpt-5.6-sol");
        assert_eq!(
            (
                sol.cost.input,
                sol.cost.output,
                sol.cost.cache_read,
                sol.cost.cache_write
            ),
            (5.0, 30.0, 0.5, 6.25)
        );
        // MIRROR: the non-5.6 rows keep their own windows (`CODEX_SPARK_CONTEXT` / `CODEX_CONTEXT`).
        assert_eq!(find("gpt-5.3-codex-spark").context_window, 128_000);
        assert_eq!(find("gpt-5.4").context_window, 272_000);
    }

    /// `gpt-5.4` verbatim from `openai-codex.models.ts` @`b0c2a90e`, including the long-context
    /// pricing tier that doubles every rate above 272 000 input tokens.
    #[test]
    fn gpt_5_4_matches_the_upstream_row() {
        let models = openai_codex_models();
        let m = models
            .iter()
            .find(|m| m.id.as_str() == "gpt-5.4")
            .expect("gpt-5.4");
        assert_eq!(m.name, "GPT-5.4");
        assert_eq!(m.input, vec![Modality::Text, Modality::Image]);
        assert_eq!(m.context_window, 272_000);
        assert_eq!(m.max_tokens, 128_000);
        assert_eq!(m.cost.input, 2.5);
        assert_eq!(m.cost.output, 15.0);
        assert_eq!(m.cost.cache_read, 0.25);
        assert_eq!(m.cost.cache_write, 0.0);

        let tiers = m.cost.tiers.as_ref().expect("gpt-5.4 has a long-context tier");
        assert_eq!(tiers.len(), 1);
        let tier = &tiers[0];
        assert_eq!(tier.input_tokens_above, 272_000);
        assert_eq!(tier.input, 5.0);
        assert_eq!(tier.output, 22.5);
        assert_eq!(tier.cache_read, 0.5);
        assert_eq!(tier.cache_write, 0.0);

        assert_eq!(
            m.compat.as_ref().and_then(|c| c.supports_tool_search),
            Some(true)
        );
        let map = m.thinking_level_map.as_ref().expect("thinkingLevelMap");
        assert_eq!(map.get("xhigh"), Some(&Some("xhigh".to_string())));
        assert_eq!(map.get("minimal"), Some(&Some("low".to_string())));
        assert_eq!(map.len(), 2);
    }

    /// `gpt-5.3-codex-spark` is the MIRROR row: the ONLY model without `supportsToolSearch`, the
    /// only text-only one, and the only one with neither a pricing tier nor the 372k window. So the
    /// assertions above pin real per-row data rather than a uniform catalog.
    #[test]
    fn codex_spark_is_the_odd_row_out() {
        let models = openai_codex_models();
        let spark = models
            .iter()
            .find(|m| m.id.as_str() == "gpt-5.3-codex-spark")
            .expect("gpt-5.3-codex-spark");
        assert_eq!(spark.input, vec![Modality::Text]);
        assert!(!spark.supports_image_input());
        assert!(spark.compat.is_none());
        assert!(spark.cost.tiers.is_none());
        assert_eq!(spark.context_window, 128_000);

        // Every other row opts into tool search.
        for m in models.iter().filter(|m| m.id.as_str() != "gpt-5.3-codex-spark") {
            assert_eq!(
                m.compat.as_ref().and_then(|c| c.supports_tool_search),
                Some(true),
                "{} lost supportsToolSearch",
                m.id.as_str()
            );
        }
    }

    /// The `gpt-5.6-*` trio added `max` to the thinking-level map; its window is the same 272k as
    /// the 5.4/5.5 rows (`CODEX_GPT_56_CONTEXT === CODEX_CONTEXT === 272000`).
    ///
    /// ASSERTION CORRECTED: this test previously pinned `372_000`, which no pi version ever
    /// emitted — `CODEX_GPT_56_CONTEXT` is `272000` at v0.84.1
    /// (`ai/scripts/generate-models.ts:2541`) and at v0.83.0 (`…:2352`), and it is the only value
    /// the three `gpt-5.6-*` codex literals use (v0.84.1 `…:2602`, `:2614`, `:2626`). The old
    /// expectation pinned cyrup's transposed digit, not upstream.
    #[test]
    fn the_gpt_5_6_trio_carries_the_max_thinking_level() {
        let models = openai_codex_models();
        for id in ["gpt-5.6-luna", "gpt-5.6-sol", "gpt-5.6-terra"] {
            let m = models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("{id} missing"));
            assert_eq!(m.context_window, 272_000, "{id}");
            let map = m.thinking_level_map.as_ref().expect("thinkingLevelMap");
            assert_eq!(map.get("max"), Some(&Some("max".to_string())), "{id}");
            // Unlike every earlier row, the trio bills cache WRITES.
            assert!(m.cost.cache_write > 0.0, "{id}");
        }
        // MIRROR: the 5.4/5.5 rows have no `max` level and no cache-write charge.
        for id in ["gpt-5.4", "gpt-5.4-mini", "gpt-5.5"] {
            let m = models
                .iter()
                .find(|m| m.id.as_str() == id)
                .unwrap_or_else(|| panic!("{id} missing"));
            let map = m.thinking_level_map.as_ref().expect("thinkingLevelMap");
            assert_eq!(map.get("max"), None, "{id}");
            assert_eq!(m.cost.cache_write, 0.0, "{id}");
        }
    }

    // ------------------------------------------------------------------ provider shape

    /// pi `openaiCodexProvider` (`openai-codex.ts:9-18`): id, display name, base URL, and OAuth-only
    /// auth — there is deliberately no API-key strategy for the subscription endpoint.
    #[test]
    fn provider_matches_the_upstream_factory() {
        let provider = openai_codex_provider();
        assert_eq!(provider.id().as_str(), "openai-codex");
        assert_eq!(provider.name(), "OpenAI Codex");
        assert_eq!(provider.models().len(), 7);

        let auth = provider.provider_auth().expect("codex declares auth");
        assert!(auth.oauth.is_some());
        assert!(
            auth.api_key.is_none(),
            "pi wires `auth: {{ oauth }}` only (openai-codex.ts:13-15)"
        );
        assert_eq!(
            auth.oauth.as_ref().map(|o| o.name().to_string()),
            Some("OpenAI (ChatGPT Plus/Pro)".to_string())
        );
    }

    // ------------------------------------------------------------------ atob / JWT

    /// The RFC 4648 §10 base64 test vectors, decoded. These are the spec's, not this code's.
    #[test]
    fn atob_decodes_the_rfc4648_vectors() {
        for (encoded, decoded) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9vYmE=", "fooba"),
            ("Zm9vYmFy", "foobar"),
        ] {
            assert_eq!(
                atob(encoded).as_deref(),
                Some(decoded.as_bytes()),
                "RFC 4648 vector {encoded:?}"
            );
        }
    }

    /// `atob`'s acceptance set, which is what makes upstream's JWT decode strict (see [`atob`]).
    #[test]
    fn atob_matches_the_html_algorithm() {
        // Unpadded input is accepted — a JWT payload never carries `=`.
        assert_eq!(atob("Zg").as_deref(), Some(&b"f"[..]));
        assert_eq!(atob("Zm8").as_deref(), Some(&b"fo"[..]));
        // ASCII whitespace is stripped before decoding.
        assert_eq!(atob("Zm 9v\nYmFy").as_deref(), Some(&b"foobar"[..]));
        // `n % 4 == 1` is not a base64 length.
        assert_eq!(atob("Z"), None);
        assert_eq!(atob("Zm9vY"), None);
        // The base64URL alphabet is NOT accepted — this is the sharp edge upstream inherits from
        // `atob`, and the reason a payload encoding to index 62/63 fails the whole refresh.
        assert_eq!(atob("-w"), None);
        assert_eq!(atob("_w"), None);
        // ...while the STANDARD spellings of the same two values are fine.
        assert!(atob("+w").is_some());
        assert!(atob("/w").is_some());
    }

    /// Build a JWT with the given payload, base64-STANDARD encoded the way `atob` expects.
    fn jwt(payload: &str) -> String {
        let engine = base64::engine::general_purpose::STANDARD;
        format!(
            "{}.{}.{}",
            engine.encode(r#"{"alg":"RS256","typ":"JWT"}"#),
            engine.encode(payload),
            engine.encode("signature")
        )
    }

    fn account_jwt(account_id: &str) -> String {
        jwt(&format!(
            r#"{{"https://api.openai.com/auth":{{"chatgpt_account_id":"{account_id}"}}}}"#
        ))
    }

    /// pi `getAccountId` (`:377-382`): the namespaced claim is read, and every way of missing it
    /// yields `null`.
    #[test]
    fn account_id_comes_from_the_namespaced_claim() {
        assert_eq!(
            openai_codex_account_id(&account_jwt("acct_9d3f")),
            Some("acct_9d3f".to_string())
        );

        // No claim object at all.
        assert_eq!(openai_codex_account_id(&jwt(r#"{"sub":"user_1"}"#)), None);
        // Claim present, account id missing.
        assert_eq!(
            openai_codex_account_id(&jwt(r#"{"https://api.openai.com/auth":{}}"#)),
            None
        );
        // `typeof accountId === "string"` — a number is not one (`:381`).
        assert_eq!(
            openai_codex_account_id(&jwt(
                r#"{"https://api.openai.com/auth":{"chatgpt_account_id":7}}"#
            )),
            None
        );
        // `accountId.length > 0` (`:381`).
        assert_eq!(openai_codex_account_id(&account_jwt("")), None);
        // `parts.length !== 3` (`:103`).
        assert_eq!(openai_codex_account_id("only.two"), None);
        assert_eq!(openai_codex_account_id("a.b.c.d"), None);
        // Not base64, and not JSON.
        assert_eq!(openai_codex_account_id("aaa.!!!!.ccc"), None);
        assert_eq!(
            openai_codex_account_id(&jwt("not json at all")),
            None
        );
    }

    /// A base64**url** payload — one character outside the STANDARD alphabet — makes upstream's
    /// `atob` throw and `decodeJwt` return `null`. MIRROR: the untouched token resolves, so this
    /// asserts the alphabet and not "any token fails".
    #[test]
    fn a_base64url_payload_defeats_the_account_id_lookup() {
        let token = account_jwt("acct_9d3f");
        assert!(openai_codex_account_id(&token).is_some(), "MIRROR");

        let mut parts = token.split('.');
        let header = parts.next().unwrap();
        let payload = parts.next().unwrap();
        let signature = parts.next().unwrap();
        // Swap one payload character for the base64url-only `_`.
        let mutated: String = payload
            .char_indices()
            .map(|(i, c)| if i == 4 { '_' } else { c })
            .collect();
        assert_ne!(mutated, payload);
        assert_eq!(
            openai_codex_account_id(&format!("{header}.{mutated}.{signature}")),
            None
        );
    }

    // ------------------------------------------------------------------ toAuth

    fn codex_credential(access: &str) -> Credential {
        Credential::Oauth {
            refresh: "rt_stored".to_string(),
            access: access.to_string(),
            expires: 0,
            ext: Map::new(),
        }
    }

    /// pi `toAuth` (`:458-460`): the access token is the bearer, and NOTHING else is set — no
    /// headers, and no base-url override (unlike GitHub Copilot, whose endpoint is credential-
    /// derived).
    #[tokio::test]
    async fn to_auth_is_the_bare_bearer() {
        let auth = OpenAiCodexOAuth::new()
            .to_auth(&codex_credential("jwt-access-token"))
            .await
            .expect("to_auth");
        assert_eq!(auth.api_key.as_deref(), Some("jwt-access-token"));
        assert!(auth.headers.is_none());
        assert!(auth.base_url.is_none());
    }

    // ------------------------------------------------------------------ refresh (loopback only)

    /// A loopback HTTP server that answers any request with `status`/`body` and records the request
    /// head + body it saw. Nothing here can reach a real host.
    async fn spawn_token_endpoint(
        status: u16,
        reason: &'static str,
        body: &'static str,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let sink = sink.clone();
                tokio::spawn(async move {
                    let mut raw: Vec<u8> = Vec::new();
                    let mut buf = [0u8; 512];
                    // Read the head, then exactly `Content-Length` more bytes.
                    loop {
                        match sock.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                raw.extend_from_slice(buf.get(..n).unwrap_or(&[]));
                                let text = String::from_utf8_lossy(&raw).to_string();
                                if let Some(head_end) = text.find("\r\n\r\n") {
                                    let want: usize = text
                                        .to_ascii_lowercase()
                                        .split("content-length:")
                                        .nth(1)
                                        .and_then(|rest| {
                                            rest.split("\r\n").next().map(str::trim).and_then(|v| {
                                                v.parse::<usize>().ok()
                                            })
                                        })
                                        .unwrap_or(0);
                                    if raw.len() >= head_end + 4 + want {
                                        break;
                                    }
                                }
                            }
                            Err(_) => return,
                        }
                    }
                    if let Ok(mut g) = sink.lock() {
                        g.push(String::from_utf8_lossy(&raw).to_string());
                    }
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn stored_codex_credential() -> Credential {
        Credential::Oauth {
            refresh: "rt_stored_refresh_token".to_string(),
            access: "stale-jwt".to_string(),
            expires: 0,
            ext: Map::new(),
        }
    }

    /// The whole of pi `refreshOpenAICodexToken` (`:399-401`): a form-encoded
    /// `grant_type=refresh_token` POST, the rotated refresh token kept, `Date.now() + expires_in *
    /// 1000` stamped, and the account id lifted out of the new JWT onto the credential.
    #[tokio::test]
    async fn refresh_exchanges_the_refresh_token_for_a_chatgpt_jwt() {
        // The JWT the endpoint returns is built here so the account id is a real claim.
        let access = account_jwt("acct_5f21");
        let body: &'static str = Box::leak(
            format!(
                r#"{{"access_token":"{access}","refresh_token":"rt_rotated","expires_in":3600}}"#
            )
            .into_boxed_str(),
        );
        let (origin, seen) = spawn_token_endpoint(200, "OK", body).await;
        let oauth = OpenAiCodexOAuth::new()
            .with_auth_context(empty_env())
            .with_token_url(&format!("{origin}/oauth/token"));

        let before = now_ms();
        let refreshed = oauth
            .refresh(&stored_codex_credential())
            .await
            .expect("refresh");
        let after = now_ms();

        let Credential::Oauth {
            refresh,
            access: new_access,
            expires,
            ext,
        } = refreshed
        else {
            panic!("refresh must yield an oauth credential");
        };
        // pi returns the ROTATED refresh token, not the one it sent (`:141`).
        assert_eq!(refresh, "rt_rotated");
        assert_eq!(new_access, access);
        // `Date.now() + expires_in * 1000` (`:144`).
        assert!(expires >= before + 3_600_000 && expires <= after + 3_600_000);
        assert_eq!(
            ext.get(EXT_ACCOUNT_ID),
            Some(&Value::String("acct_5f21".to_string()))
        );

        // The request shape: POST /oauth/token, form-encoded, with upstream's three fields in
        // upstream's order (`:168-172`).
        let requests = seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(request.starts_with("POST /oauth/token "), "{request}");
        assert!(
            request
                .to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded")
        );
        assert!(
            request.ends_with(
                "grant_type=refresh_token&refresh_token=rt_stored_refresh_token\
                 &client_id=app_EMoamEEZ73f0CkXaXp7hrann"
            ),
            "unexpected body in {request}"
        );
    }

    /// pi `:124-126`: a non-2xx answer is `"OpenAI Codex token refresh failed (<status>): <body>"`,
    /// verbatim — the `refresh` operation word, the numeric status, and the raw body.
    #[tokio::test]
    async fn a_non_2xx_token_response_is_the_upstream_error_string() {
        let (origin, _) = spawn_token_endpoint(
            401,
            "Unauthorized",
            r#"{"error":"invalid_grant"}"#,
        )
        .await;
        let oauth = OpenAiCodexOAuth::new()
            .with_auth_context(empty_env())
            .with_token_url(&format!("{origin}/oauth/token"));

        let err = oauth
            .refresh(&stored_codex_credential())
            .await
            .expect_err("must fail");
        assert_eq!(err.code(), "oauth");
        let cause = std::error::Error::source(&err).expect("cause");
        assert_eq!(
            cause.to_string(),
            r#"OpenAI Codex token refresh failed (401): {"error":"invalid_grant"}"#
        );
    }

    /// pi `:136-138`: a 2xx body missing any of the three fields is
    /// `"OpenAI Codex token refresh response missing fields: <json>"`. JS truthiness means an EMPTY
    /// `access_token`/`refresh_token` is missing, and `expires_in` must be a *number*.
    #[tokio::test]
    async fn a_malformed_token_body_is_the_upstream_missing_fields_error() {
        for body in [
            r#"{"refresh_token":"rt","expires_in":3600}"#,
            r#"{"access_token":"at","expires_in":3600}"#,
            r#"{"access_token":"at","refresh_token":"rt"}"#,
            r#"{"access_token":"at","refresh_token":"rt","expires_in":"3600"}"#,
            r#"{"access_token":"","refresh_token":"rt","expires_in":3600}"#,
        ] {
            let (origin, _) = spawn_token_endpoint(200, "OK", body).await;
            let oauth = OpenAiCodexOAuth::new()
                .with_auth_context(empty_env())
                .with_token_url(&format!("{origin}/oauth/token"));
            let err = oauth
                .refresh(&stored_codex_credential())
                .await
                .expect_err("must fail");
            let cause = std::error::Error::source(&err).expect("cause");
            assert!(
                cause
                    .to_string()
                    .starts_with("OpenAI Codex token refresh response missing fields: {"),
                "body {body} gave {cause}"
            );
        }
    }

    /// pi `:386`: a token whose payload has no `chatgpt_account_id` fails the whole refresh with
    /// this exact string — the credential is never persisted half-formed.
    #[tokio::test]
    async fn a_token_without_an_account_id_claim_fails_the_refresh() {
        let access = jwt(r#"{"sub":"user_1"}"#);
        let body: &'static str = Box::leak(
            format!(
                r#"{{"access_token":"{access}","refresh_token":"rt_rotated","expires_in":3600}}"#
            )
            .into_boxed_str(),
        );
        let (origin, _) = spawn_token_endpoint(200, "OK", body).await;
        let oauth = OpenAiCodexOAuth::new()
            .with_auth_context(empty_env())
            .with_token_url(&format!("{origin}/oauth/token"));

        let err = oauth
            .refresh(&stored_codex_credential())
            .await
            .expect_err("must fail");
        let cause = std::error::Error::source(&err).expect("cause");
        assert_eq!(cause.to_string(), "Failed to extract accountId from token");
    }
}
