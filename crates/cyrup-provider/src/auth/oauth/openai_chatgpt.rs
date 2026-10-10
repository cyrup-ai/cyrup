//! "Sign in with ChatGPT" on the plain `openai` provider — port of pi v1.0.1
//! `packages/ai/src/auth/oauth/openai-chatgpt.ts` (309 lines), added by `02eed88fd` ("add
//! alternative sign in for the openai provider") and changed by `eeac84ca9` at v1.0.1.
//!
//! Upstream's header: "OpenAI Responses API token sharing through Sign in with ChatGPT. This
//! public-client flow uses no client secret and sends the resulting user access token directly to
//! api.openai.com." (`openai-chatgpt.ts:1-6`).
//!
//! This is **not** [`super::openai_codex`]. Both are ChatGPT-subscription logins against
//! `auth.openai.com`, but:
//!
//! | | `openai_codex` (legacy) | this flow |
//! |---|---|---|
//! | client | fixed `app_EMoamEEZ73f0CkXaXp7hrann` | **dynamic registration**: every login sends `client_id=dynamic_agent_client` and reads the issued id back out of the callback's `client_id` parameter |
//! | authorize | `auth.openai.com/oauth/authorize` | `auth.openai.com/api/accounts/authorize` |
//! | token | `auth.openai.com/oauth/token` | `auth.openai.com/api/accounts/oauth/token` |
//! | target | `chatgpt.com/backend-api/codex` | `api.openai.com/v1` (`resource=`) |
//! | credential | access token + `chatgpt_account_id` JWT claim | access token + issued `clientId` + granted `scopes` |
//! | expiry skew | none (`openai-codex.ts:144`) | three minutes ([`EXPIRY_MARGIN_MS`]) |
//! | bind failure | degrades to manual paste (`openai-codex.ts:361-370`) | **hard-fails** ([`PORT_IN_USE_MESSAGE`]) |
//!
//! ## Provenance
//!
//! | this module | `openai-chatgpt.ts` @v1.0.1 |
//! |---|---|
//! | [`DYNAMIC_CLIENT_ID`]/[`AGENT_NAME_HINT`]/[`UUID_PATTERN_LEN`] | `:16-18` |
//! | [`AUTHORIZE_URL`]/[`TOKEN_URL`]/[`RESOURCE`] | `:19-21` |
//! | [`CALLBACK_PORT`]/[`CALLBACK_PATH`]/[`REDIRECT_URI`] | `:23-25` |
//! | [`DIRECT_TOKEN_SCOPE`]/[`SCOPE`]/[`EXPIRY_MARGIN_MS`] | `:26-29` |
//! | [`AuthorizationResult`] | `type AuthorizationResult`, `:31-34` |
//! | [`random_value`] | `randomValue`, `:49-51` |
//! | [`authorization_result_from_callback`] | `authorizationResultFromCallback`, `:53-62` |
//! | [`authorization_result_from_manual_input`] | `authorizationResultFromManualInput`, `:64-78` |
//! | [`ChatGptCallbackHandler`] | the `createServer` handler, `:94-123` |
//! | [`OpenAiChatGptOAuth::start_callback_server`] | `startCallbackServer`, `:85-132` + `:243-248` |
//! | [`OpenAiChatGptOAuth::request_token`] | `requestToken`, `:134-153` |
//! | [`require_token_string`] | `requireTokenString`, `:155-160` |
//! | [`credential_from_token_response`] | `credentialFromTokenResponse`, `:162-181` |
//! | [`OpenAiChatGptOAuth::exchange_authorization_code`] | `exchangeAuthorizationCode`, `:183-206` |
//! | [`OpenAiChatGptOAuth::refresh_access_token`] | `refreshAccessToken`, `:208-223` |
//! | [`agent_host_id`] | `agentHostId`, `:225-231` |
//! | [`OpenAiChatGptOAuth::run_login`] | `loginOpenAIChatGPT`, `:233-298` |
//! | `impl OAuthAuth for OpenAiChatGptOAuth` | `openaiChatGPTOAuth`, `:300-309` |
//!
//! ## The port-1455 collision
//!
//! Both OpenAI logins bind **the same port and the same path**: this flow's [`REDIRECT_URI`] is
//! `http://127.0.0.1:1455/auth/callback` (`:23-25`) and Codex's is
//! `http://localhost:1455/auth/callback` (`openai-codex.ts:26`, listened on `:365`) — one loopback
//! address under two spellings. v1.0.1 split their bind-failure behaviour deliberately:
//!
//! * Codex keeps degrading to paste, and its comment says why the port is contended —
//!   *"Port 1455 is shared with the Codex CLI; when it is taken, fall back to the pasted redirect
//!   URL"* (`openai-codex.ts:361`);
//! * this flow **hard-fails** with [`PORT_IN_USE_MESSAGE`] and rethrows any other bind error
//!   (`:243-248`), because its callback carries the *issued client id* — a pasted URL from a
//!   browser whose redirect landed on the other listener is rejected there as a state mismatch and
//!   never reaches this login (upstream's own comment, `:241-242`).
//!
//! What v1.0.1 dropped is the **degrade-to-paste on bind failure**, not the paste branch: the
//! `manual_code` prompt at `:271-279` is still raced against the callback on every successful bind
//! (see the `v1.0.0..v1.0.1` diff, which replaces `try { callback = await
//! startCallbackServer(state) } catch { interaction.notify({type:"info", …}) }` with a
//! `.catch()` that throws). `PROV-117`'s degrade-to-paste pattern must therefore NOT be applied to
//! the bind failure here.
//!
//! ## Mechanism divergences (Rust forces these; behaviour is unchanged unless noted)
//!
//! * **Callback server.** Upstream calls `node:http.createServer` (`:94`); this crate uses the
//!   shared [`super::callback::CallbackServer`], as every other ported flow does. Three
//!   consequences, all of them already recorded for [`super::openai_codex`]:
//!   1. a foreign route answers `"OAuth callback route not found."` where `:98` says
//!      `"Callback route not found."` — the shared server owns that page;
//!   2. `:121`'s 500 `"Internal error while processing the callback."` is unreachable: a Rust
//!      handler returns a [`CallbackOutcome`] and cannot throw;
//!   3. `:296`'s `closeAllConnections()` has no counterpart — it exists to stop a *browser's
//!      pre-opened spare connection* being served by a previous login's `node:http` server, and
//!      the shared listener drops every connection when its accept loop stops.
//! * **No ambient `fetch`.** Requests go through `reqwest`; `AbortSignal` is a [`CancelToken`]
//!   (arch-00 §3.2) raced against the request rather than handed to it.
//! * **`PI_OAUTH_CALLBACK_HOST` is read per login, not at module load.** Upstream's `CALLBACK_HOST`
//!   is a module-level `const` (`:22`), so the value is frozen at first import; cyrup's
//!   [`super::callback::callback_host`] is async and resolves `CYRUP_OAUTH_CALLBACK_HOST` when the
//!   login starts, exactly as `openai-codex.ts:40-46`'s per-login `getCallbackHost()` does. The
//!   only observable difference is a process that mutates its own environment between two logins.
//! * **`agent_name_hint: "Pi"`** (`:17`) is sent verbatim, for the reason
//!   [`super::xai::REFERRER`] and [`super::openai_codex::ORIGINATOR`] are: it is a value OpenAI's
//!   authorization server reads, paired with [`DYNAMIC_CLIENT_ID`], not a cyrup-facing brand
//!   string. Rebranding it would change what the consent screen is told it is approving.
//!   It is the default only: an embedding app that sets `LoginOptions::agent_name` is named
//!   instead, as `options?.agentName ?? AGENT_NAME_HINT` does upstream (`:253` @f1b2e77f5,
//!   PROV-144).
//! * **Endpoint override.** Upstream's tests stub the ambient `fetch`; Rust has none, so every
//!   endpoint is a field of [`OpenAiChatGptEndpoints`], defaulting to the upstream constants.
//!   Production callers use [`OpenAiChatGptOAuth::new`].
//!
//! ## Secrets
//!
//! No access token, refresh token, authorization code or device id appears in any message, event or
//! `Debug` rendering produced by this module. [`OpenAiChatGptEndpoints`] derives `Debug` (it holds
//! only URLs); the credential itself is [`Credential`], whose redacting `Debug` is pinned by
//! `auth::types`' own tests. The token-request failure string reproduces upstream's
//! `(${status}): ${body}` (`:146`) — the body of a *failed* token call, which carries an OAuth
//! error code, never a granted token.

use super::callback::{
    CallbackControl, CallbackHandler, CallbackOutcome, CallbackReply, CallbackRequest,
    CallbackServer, CallbackServerConfig, callback_host,
};
use super::interaction::{AuthEvent, AuthInteraction, AuthPrompt};
use super::pkce::generate_pkce;
use super::query::encode_query;
use super::random::random_token;
use super::{OAuthError, now_ms};
use crate::auth::types::{Credential, EnvAuthContext, ModelAuth};
use crate::auth::{LoginOptions, OAuthAuth};
use crate::error::AuthError;
use cyrup_core::CancelToken;

// ---------------------------------------------------------------------------
// Constants — openai-chatgpt.ts:16-29
// ---------------------------------------------------------------------------

/// `DYNAMIC_CLIENT_ID` (`:16`). Upstream's comment: "every login registers a new client with this
/// ID; OpenAI returns the issued client ID in the callback" (`:15`).
pub const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
/// `AGENT_NAME_HINT` (`:17`) — sent verbatim; see this module's divergence note.
pub const AGENT_NAME_HINT: &str = "Pi";
/// `AUTHORIZE_URL` (`:19`).
pub const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
/// `TOKEN_URL` (`:20`).
pub const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
/// `RESOURCE` (`:21`) — the audience the issued token is scoped to, and the base URL the token is
/// then sent to. Keep in step with [`crate::providers::openai::OPENAI_BASE_URL`]: the request
/// shaping in [`crate::api::openai_responses`] keys on that pairing.
pub const RESOURCE: &str = "https://api.openai.com/v1";
/// `CALLBACK_PORT` (`:23`) — **shared with [`super::openai_codex::CALLBACK_PORT`]**; see this
/// module's collision note.
pub const CALLBACK_PORT: u16 = 1455;
/// `CALLBACK_PATH` (`:24`).
pub const CALLBACK_PATH: &str = "/auth/callback";
/// `REDIRECT_URI` (`:25`). Pre-registered, so the listener cannot use an ephemeral port. Note the
/// host is the literal `127.0.0.1`, where `openai-codex.ts:26` registers `localhost`.
pub const REDIRECT_URI: &str = "http://127.0.0.1:1455/auth/callback";
/// `DIRECT_TOKEN_SCOPE` (`:26`) — the grant that makes the access token usable against
/// `api.openai.com` directly. A grant without it is refused (`:170-172`).
pub const DIRECT_TOKEN_SCOPE: &str = "chatgpt.tokens.use.direct";
/// `SCOPE` (`:27`).
pub const SCOPE: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// `EXPIRY_MARGIN_MS = 3 * 60 * 1000` (`:29`). Upstream's comment: "Refresh this long before the
/// real expiry so a request never starts with a token about to expire."
pub const EXPIRY_MARGIN_MS: i64 = 3 * 60 * 1000;

/// The display name of this strategy (`:301`).
pub const OPENAI_CHATGPT_OAUTH_NAME: &str = "OpenAI (ChatGPT subscription)";
/// `loginLabel` (`:303`) — how `/login` distinguishes this from the legacy Codex provider.
pub const OPENAI_CHATGPT_LOGIN_LABEL: &str = "Sign in with ChatGPT";

/// The `ext` key the issued client id is stored under (`credential.clientId`, `:178`). Camel-case
/// because the on-disk `auth.json` is pi's (R-00-013).
pub const EXT_CLIENT_ID: &str = "clientId";
/// The `ext` key the granted scopes are stored under (`credential.scopes`, `:179`).
pub const EXT_SCOPES: &str = "scopes";

/// `:246-247`, verbatim. This is the whole of v1.0.1's behavioural change: a contended port is a
/// **failure with a remedy**, not a degrade.
///
/// The number is upstream's `${CALLBACK_PORT}` — the module constant, not whatever port the
/// listener tried to bind — so a test that points [`OpenAiChatGptEndpoints::callback_port`]
/// elsewhere still reads `1455`, exactly as upstream would.
///
/// The word "pi" is upstream's and is kept verbatim per this change's no-concession rule. It is the
/// one user-facing string in this module that names the upstream product rather than a
/// wire-visible identifier; compare [`super::callback::callback_host`], where the *config* name
/// `PI_OAUTH_CALLBACK_HOST` was rebranded to `CYRUP_OAUTH_CALLBACK_HOST`. Rewording it is a
/// product decision, not a port decision.
pub const PORT_IN_USE_MESSAGE: &str = "Port 1455 is in use, probably by an unfinished login in another pi session or by the Codex CLI. Cancel that login and try again.";

/// `:55`.
const MISSING_CODE_MESSAGE: &str = "Missing authorization code";
/// `:57`.
const MISSING_STATE_MESSAGE: &str = "Missing OAuth state";
/// `:58` — note this is `"OAuth state mismatch"`, matching `anthropic.ts` and **not**
/// `openai-codex.ts`'s bare `"State mismatch"`.
const STATE_MISMATCH_MESSAGE: &str = "OAuth state mismatch";
/// `:60`.
const MISSING_CLIENT_ID_MESSAGE: &str =
    "OpenAI OAuth registration callback did not contain an issued client ID";
/// `:69`.
const PASTE_FULL_URL_MESSAGE: &str = "Paste the full callback URL from the browser";
/// `:228`.
const MISSING_DEVICE_ID_MESSAGE: &str =
    "Sign in with ChatGPT requires a device ID (UUID) for this installation";
/// `:118`.
const CALLBACK_SUCCESS_MESSAGE: &str =
    "ChatGPT authentication completed. You can close this window.";
/// `:104`.
const CALLBACK_ERROR_MESSAGE: &str = "ChatGPT was not connected.";
/// `:267-268`.
const AUTH_URL_INSTRUCTIONS: &str = "Complete sign-in in your browser. If the callback does not complete, paste the final redirect URL here.";
/// `:275`.
const MANUAL_PROMPT_MESSAGE: &str =
    "Complete login in your browser, or paste the final redirect URL here:";
/// `:283`.
const EXCHANGING_MESSAGE: &str = "Exchanging authorization code for tokens...";
/// `:211`.
const NO_STORED_CLIENT_ID_MESSAGE: &str =
    "Stored OpenAI OAuth credential does not contain an issued client ID; reconnect ChatGPT";
/// `:203`.
const NO_ID_TOKEN_MESSAGE: &str = "OpenAI OAuth token response did not contain an ID token";
/// `:150`.
const NON_OBJECT_TOKEN_MESSAGE: &str = "OpenAI OAuth token response must be an object";

/// The character length of `UUID_PATTERN` (`:18`) matches — 8-4-4-4-12 plus four hyphens.
pub const UUID_PATTERN_LEN: usize = 36;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// `randomValue()` (`:49-51`): `randomBytes(32).toString("base64url")`.
fn random_value() -> Result<String, OAuthError> {
    random_token(32)
}

/// `UUID_PATTERN.test(deviceId)` (`:18`, applied `:227`) — the 8-4-4-4-12 hex form, case
/// insensitive. Written out rather than compiled as a regex because the shape is fixed.
fn is_uuid(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    if value.len() != UUID_PATTERN_LEN {
        return false;
    }
    let mut rest = value;
    for (index, width) in GROUPS.iter().enumerate() {
        if index > 0 {
            match rest.strip_prefix('-') {
                Some(tail) => rest = tail,
                None => return false,
            }
        }
        if rest.len() < *width {
            return false;
        }
        let (group, tail) = rest.split_at(*width);
        if !group.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
        rest = tail;
    }
    rest.is_empty()
}

/// `agentHostId` (`:225-231`). Upstream's doc comment: "OpenAI identifies each installation
/// ("agent host") by a stable URI such as `urn:uuid:<uuid>`."
///
/// This is the whole answer to "what is `getDeviceId` for": the app's *installation* id, created on
/// first use and returned identically thereafter ([`LoginOptions::get_device_id`]), lowercased into
/// a URN and sent as the authorize request's `ext_agent_host_id`. It is **not** a secret the user
/// types and not an RFC 8628 device code; it is how OpenAI recognises repeat logins from the same
/// install. A missing or non-UUID id fails the login **before** any authorization starts (`:237`
/// runs ahead of [`Self::start_callback_server`]), which is what upstream's own test
/// `"requires a device ID before starting authorization"` pins.
fn agent_host_id(device_id: Option<&str>) -> Result<String, OAuthError> {
    match device_id {
        Some(id) if is_uuid(id) => Ok(format!("urn:uuid:{}", id.to_lowercase())),
        _ => Err(OAuthError::Failed(MISSING_DEVICE_ID_MESSAGE.to_string())),
    }
}

/// `{ code, clientId }` (`type AuthorizationResult`, `:31-34`) — the callback yields **two**
/// values, because the client id is minted per login by dynamic registration.
///
/// `Debug` is hand-written and redacts `code`: a one-time authorization code is exchangeable
/// credential material, and this value is carried through a `select!` and a `Result`, both of which
/// a future `{:?}` could render. `client_id` is not a secret — it is the id OpenAI issued for this
/// install and the thing a support conversation needs to name.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthorizationResult {
    pub code: String,
    pub client_id: String,
}

impl std::fmt::Debug for AuthorizationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizationResult")
            .field("code", &crate::auth::types::REDACTED)
            .field("client_id", &self.client_id)
            .finish()
    }
}

/// `authorizationResultFromCallback` (`:53-62`), over already-parsed query pairs.
///
/// Pure, so the four rejection branches are assertable without a listener. The order is upstream's:
/// code, state presence, state equality, client id.
pub fn authorization_result_from_callback(
    params: &[(String, String)],
    expected_state: &str,
) -> Result<AuthorizationResult, OAuthError> {
    let get = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    // `:54-55` — `searchParams.get()` yields `""` for a bare `?code=`, which JS truthiness reads
    // as missing.
    let code = get("code").filter(|s| !s.is_empty());
    let Some(code) = code else {
        return Err(OAuthError::Failed(MISSING_CODE_MESSAGE.to_string()));
    };
    // `:56-57`
    let state = get("state").filter(|s| !s.is_empty());
    let Some(state) = state else {
        return Err(OAuthError::Failed(MISSING_STATE_MESSAGE.to_string()));
    };
    // `:58`
    if state != expected_state {
        return Err(OAuthError::Failed(STATE_MISMATCH_MESSAGE.to_string()));
    }
    // `:59-60` — `.trim()` then truthiness, so a whitespace-only `client_id` is missing.
    let client_id = get("client_id").map(str::trim).filter(|s| !s.is_empty());
    let Some(client_id) = client_id else {
        return Err(OAuthError::Failed(MISSING_CLIENT_ID_MESSAGE.to_string()));
    };
    Ok(AuthorizationResult {
        code: code.to_string(),
        client_id: client_id.to_string(),
    })
}

/// `authorizationResultFromManualInput` (`:64-78`): the pasted redirect URL must be a URL, must
/// match [`REDIRECT_URI`]'s origin **and** path, must not carry an `error`, and then goes through
/// the same checks as a real callback.
pub fn authorization_result_from_manual_input(
    input: &str,
    expected_state: &str,
    redirect_uri: &str,
) -> Result<AuthorizationResult, OAuthError> {
    // `:66-70`
    let Ok(url) = reqwest::Url::parse(input.trim()) else {
        return Err(OAuthError::Failed(PASTE_FULL_URL_MESSAGE.to_string()));
    };
    let Ok(expected) = reqwest::Url::parse(redirect_uri) else {
        return Err(OAuthError::Failed(PASTE_FULL_URL_MESSAGE.to_string()));
    };
    // `:71-74`
    if url.origin() != expected.origin() || url.path() != expected.path() {
        return Err(OAuthError::Failed(format!(
            "The pasted callback URL must start with {redirect_uri}"
        )));
    }
    let params: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    // `:75-76`
    if let Some((_, error)) = params.iter().find(|(k, v)| k == "error" && !v.is_empty()) {
        return Err(OAuthError::Failed(format!(
            "ChatGPT authorization failed: {error}"
        )));
    }
    // `:77`
    authorization_result_from_callback(&params, expected_state)
}

/// `requireTokenString` (`:155-160`).
fn require_token_string(
    value: Option<&serde_json::Value>,
    field: &str,
) -> Result<String, OAuthError> {
    match value.and_then(serde_json::Value::as_str) {
        Some(text) if !text.trim().is_empty() => Ok(text.to_string()),
        _ => Err(OAuthError::Failed(format!(
            "OpenAI OAuth token response has invalid {field}"
        ))),
    }
}

/// `credentialFromTokenResponse` (`:162-181`).
///
/// `expires` is `Date.now() + expires_in * 1000 - EXPIRY_MARGIN_MS` (`:177`): the stored deadline is
/// three minutes *before* the real one, so [`crate::auth::resolve`]'s clock refreshes in time.
pub fn credential_from_token_response(
    token: &serde_json::Value,
    client_id: &str,
) -> Result<Credential, OAuthError> {
    // `:163-165`
    let access = require_token_string(token.get("access_token"), "access_token")?;
    let refresh = require_token_string(token.get("refresh_token"), "refresh_token")?;
    let scope = require_token_string(token.get("scope"), "scope")?;
    // `:166-168` — `typeof !== "number" || !Number.isFinite || <= 0`. A numeric *string* fails.
    let expires_in = match token.get("expires_in").and_then(serde_json::Value::as_f64) {
        Some(seconds) if seconds.is_finite() && seconds > 0.0 => seconds,
        _ => {
            return Err(OAuthError::Failed(
                "OpenAI OAuth token response has invalid expires_in".to_string(),
            ));
        }
    };
    // `:169` — `scope.trim().split(/\s+/).filter(Boolean)`.
    let scopes: Vec<String> = scope.split_whitespace().map(str::to_string).collect();
    // `:170-172`
    if !scopes.iter().any(|s| s == DIRECT_TOKEN_SCOPE) {
        return Err(OAuthError::Failed(format!(
            "OpenAI OAuth grant did not include {DIRECT_TOKEN_SCOPE}"
        )));
    }
    // `:173-180`
    let mut ext = serde_json::Map::new();
    ext.insert(
        EXT_CLIENT_ID.to_string(),
        serde_json::Value::String(client_id.to_string()),
    );
    ext.insert(
        EXT_SCOPES.to_string(),
        serde_json::Value::Array(scopes.into_iter().map(serde_json::Value::String).collect()),
    );
    Ok(Credential::Oauth {
        refresh,
        access,
        expires: now_ms()
            .saturating_add((expires_in * 1000.0) as i64)
            .saturating_sub(EXPIRY_MARGIN_MS),
        ext,
    })
}

/// `credential.clientId` (`:209-212`) — the issued client id a stored credential must carry for
/// its refresh to be addressable.
pub fn stored_client_id(cred: &Credential) -> Option<&str> {
    match cred {
        Credential::Oauth { ext, .. } => ext
            .get(EXT_CLIENT_ID)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        Credential::ApiKey { .. } => None,
    }
}

// ---------------------------------------------------------------------------
// The callback handler — openai-chatgpt.ts:94-123
// ---------------------------------------------------------------------------

/// The ChatGPT half of the callback server (`:94-123`).
///
/// Note the asymmetry upstream relies on: an `error=` parameter **settles** the wait with a failure
/// (`rejectResult`, `:105`), while every other rejection replies 400 and keeps listening
/// (plain `return`, `:115`).
pub struct ChatGptCallbackHandler {
    expected_state: String,
}

#[async_trait::async_trait]
impl CallbackHandler for ChatGptCallbackHandler {
    type Value = AuthorizationResult;

    async fn handle(
        &self,
        request: CallbackRequest,
        _control: CallbackControl,
    ) -> CallbackOutcome<AuthorizationResult> {
        // `:97-100` — the route check is the shared server's; see the module divergence note.
        // `:102-107`
        if let Some(error) = request.param("error").filter(|s| !s.is_empty()) {
            return CallbackOutcome::Failed {
                reply: CallbackReply::error(
                    400,
                    CALLBACK_ERROR_MESSAGE,
                    Some(&format!("Error: {error}")),
                ),
                error: OAuthError::Failed(format!("ChatGPT authorization failed: {error}")),
            };
        }
        // `:109-116`
        match authorization_result_from_callback(&request.query, &self.expected_state) {
            Ok(result) => CallbackOutcome::Complete {
                // `:118`
                reply: CallbackReply::success(CALLBACK_SUCCESS_MESSAGE),
                value: result,
            },
            Err(error) => CallbackOutcome::Continue {
                reply: CallbackReply::error(400, &error.to_string(), None),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// openaiChatGPTOAuth — openai-chatgpt.ts:300-309
// ---------------------------------------------------------------------------

/// Every endpoint the flow talks to, so a test can point them at loopback listeners.
///
/// Exists only because Rust has no ambient `fetch` for a test to stub; [`Default`] is upstream's
/// `:19-25` constants verbatim and is what [`OpenAiChatGptOAuth::new`] uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiChatGptEndpoints {
    pub authorize_url: String,
    pub token_url: String,
    pub resource: String,
    /// The **registered** redirect (`:25`), sent in the authorize URL and in the token exchange,
    /// and the origin/path a pasted URL is validated against. Upstream uses the constant regardless
    /// of what the listener bound, and so does this port.
    pub redirect_uri: String,
    /// `None` resolves `CYRUP_OAUTH_CALLBACK_HOST` at login time; see the module divergence note.
    pub callback_host: Option<String>,
    /// `0` binds an ephemeral port; production is [`CALLBACK_PORT`].
    pub callback_port: u16,
}

impl Default for OpenAiChatGptEndpoints {
    fn default() -> Self {
        Self {
            authorize_url: AUTHORIZE_URL.to_string(),
            token_url: TOKEN_URL.to_string(),
            resource: RESOURCE.to_string(),
            redirect_uri: REDIRECT_URI.to_string(),
            callback_host: None,
            callback_port: CALLBACK_PORT,
        }
    }
}

/// `openaiChatGPTOAuth` (`:300-309`).
pub struct OpenAiChatGptOAuth {
    endpoints: OpenAiChatGptEndpoints,
    agent_name_hint: String,
}

impl Default for OpenAiChatGptOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiChatGptOAuth {
    /// The production strategy — upstream's `:19-25` endpoints.
    pub fn new() -> Self {
        Self::with_endpoints(OpenAiChatGptEndpoints::default())
    }

    /// Point the flow at different endpoints. Test-facing seam; production code uses
    /// [`OpenAiChatGptOAuth::new`].
    pub fn with_endpoints(endpoints: OpenAiChatGptEndpoints) -> Self {
        Self {
            endpoints,
            agent_name_hint: AGENT_NAME_HINT.to_string(),
        }
    }

    /// The endpoints in force, for callers that need to build a matching authorize URL.
    pub fn endpoints(&self) -> &OpenAiChatGptEndpoints {
        &self.endpoints
    }

    /// Proxy-aware per target, as every other flow's client is (PROV-047).
    async fn client(&self, target_url: &str) -> Result<reqwest::Client, OAuthError> {
        crate::stream::sse::build_client_for(target_url)
            .await
            .map_err(|e| OAuthError::Failed(e.to_string()))
    }

    async fn bind_host(&self) -> String {
        match &self.endpoints.callback_host {
            Some(host) => host.clone(),
            None => callback_host(&EnvAuthContext, None).await,
        }
    }

    /// `startCallbackServer` (`:85-132`) with v1.0.1's `.catch` (`:243-248`).
    ///
    /// The return type is **`Result`, not `Result<Option<_>>`**: there is no "could not bind, carry
    /// on" state in this flow. `EADDRINUSE` becomes [`PORT_IN_USE_MESSAGE`] and every other bind
    /// failure propagates unchanged — upstream `if (!(… error.code === "EADDRINUSE")) throw error`.
    async fn start_callback_server(
        &self,
        expected_state: &str,
        interaction: &dyn AuthInteraction,
    ) -> Result<CallbackServer<AuthorizationResult>, OAuthError> {
        let config = CallbackServerConfig::fixed(self.endpoints.callback_port, CALLBACK_PATH)
            .with_host(self.bind_host().await)
            .with_interaction(interaction);
        CallbackServer::start(
            config,
            ChatGptCallbackHandler {
                expected_state: expected_state.to_string(),
            },
        )
        .await
        .map_err(|error| match &error {
            OAuthError::Listen { source, .. } if source.kind() == std::io::ErrorKind::AddrInUse => {
                OAuthError::Failed(PORT_IN_USE_MESSAGE.to_string())
            }
            _ => error,
        })
    }

    /// `requestToken` (`:134-153`): form-encoded POST, non-2xx becomes upstream's message, and a
    /// body that is not a JSON object is `:150`.
    async fn request_token(
        &self,
        body: String,
        cancel: Option<&CancelToken>,
    ) -> Result<serde_json::Value, OAuthError> {
        let client = self.client(&self.endpoints.token_url).await?;
        let send = client
            .post(&self.endpoints.token_url)
            .header("accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send();
        let response = match with_cancel(send, cancel).await? {
            Ok(response) => response,
            Err(error) => return Err(map_fetch_error(&error, cancel)),
        };

        let status = response.status();
        // `:144-147`
        if !status.is_success() {
            let text = with_cancel(response.text(), cancel)
                .await?
                .unwrap_or_default();
            // `${responseBody || response.statusText}` — JS truthiness.
            let detail = if text.is_empty() {
                status.canonical_reason().unwrap_or_default().to_string()
            } else {
                text
            };
            return Err(OAuthError::Failed(format!(
                "OpenAI OAuth token request failed ({}): {detail}",
                status.as_u16()
            )));
        }

        let text = match with_cancel(response.text(), cancel).await? {
            Ok(text) => text,
            Err(error) => return Err(map_fetch_error(&error, cancel)),
        };
        // `:148-152` — `typeof data !== "object" || data === null || Array.isArray(data)`. A body
        // that is not JSON at all rejects `response.json()` upstream; both land on a caller that
        // reads "the endpoint did not answer with a token", so the message is shared.
        match serde_json::from_str::<serde_json::Value>(text.trim()) {
            Ok(value) if value.is_object() => Ok(value),
            _ => Err(OAuthError::Failed(NON_OBJECT_TOKEN_MESSAGE.to_string())),
        }
    }

    /// `exchangeAuthorizationCode` (`:183-206`), in upstream's field order.
    pub async fn exchange_authorization_code(
        &self,
        code: &str,
        verifier: &str,
        client_id: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<Credential, OAuthError> {
        // `:190-197`
        let body = encode_query([
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", self.endpoints.redirect_uri.as_str()),
            ("resource", self.endpoints.resource.as_str()),
        ]);
        let token = self.request_token(body, cancel).await?;
        // `:200-204` — upstream's comment: "Pi does not use the ID token to identify the user or
        // read profile data. Keep the presence check as part of the token-response contract."
        let id_token = token
            .get("id_token")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.trim().is_empty());
        if id_token.is_none() {
            return Err(OAuthError::Failed(NO_ID_TOKEN_MESSAGE.to_string()));
        }
        credential_from_token_response(&token, client_id)
    }

    /// `refreshAccessToken` (`:208-223`). No `scope` parameter and **no** `id_token` check — a
    /// refresh response is not required to carry one.
    pub async fn refresh_access_token(
        &self,
        cred: &Credential,
        cancel: Option<&CancelToken>,
    ) -> Result<Credential, OAuthError> {
        // `:209-212`
        let Some(client_id) = stored_client_id(cred).map(str::to_string) else {
            return Err(OAuthError::Failed(NO_STORED_CLIENT_ID_MESSAGE.to_string()));
        };
        let Credential::Oauth { refresh, .. } = cred else {
            return Err(OAuthError::Failed(NO_STORED_CLIENT_ID_MESSAGE.to_string()));
        };
        // `:213-221`
        let body = encode_query([
            ("grant_type", "refresh_token"),
            ("client_id", client_id.as_str()),
            ("refresh_token", refresh.as_str()),
            ("resource", self.endpoints.resource.as_str()),
        ]);
        let token = self.request_token(body, cancel).await?;
        credential_from_token_response(&token, &client_id)
    }

    /// The authorize URL (`:250-263`). Parameter order is upstream's insertion order, which
    /// `URLSearchParams.toString()` preserves.
    fn authorization_url(
        &self,
        agent_name_hint: &str,
        host_id: &str,
        challenge: &str,
        state: &str,
        nonce: &str,
    ) -> String {
        let params = encode_query([
            ("client_id", DYNAMIC_CLIENT_ID),
            ("agent_name_hint", agent_name_hint),
            ("ext_agent_host_id", host_id),
            ("response_type", "code"),
            ("redirect_uri", self.endpoints.redirect_uri.as_str()),
            ("resource", self.endpoints.resource.as_str()),
            ("scope", SCOPE),
            ("state", state),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("nonce", nonce),
        ]);
        format!("{}?{params}", self.endpoints.authorize_url)
    }

    /// The body of `loginOpenAIChatGPT` (`:233-298`) minus its `finally`, which [`Self::login`]
    /// owns.
    async fn run_login(
        &self,
        interaction: &dyn AuthInteraction,
        server: &CallbackServer<AuthorizationResult>,
        verifier: &str,
        state: &str,
        authorization_url: String,
        manual_abort: &CancelToken,
    ) -> Result<Credential, OAuthError> {
        // `:264-269`
        interaction.notify(AuthEvent::AuthUrl {
            url: authorization_url,
            instructions: Some(AUTH_URL_INSTRUCTIONS.to_string()),
        });

        // `:271-279` — the paste branch, still present at v1.0.1 (only the *bind-failure* degrade
        // was dropped). Both futures are pinned rather than `select!`-owned because a dropped
        // `wait()` would take its result channel with it.
        let mut manual = Box::pin(
            interaction.prompt(
                AuthPrompt::manual_code(MANUAL_PROMPT_MESSAGE)
                    .with_placeholder(self.endpoints.redirect_uri.clone())
                    .with_cancel(manual_abort.clone()),
            ),
        );
        let mut waiter = Box::pin(server.wait());

        // `:282` — `Promise.race([callback.result, manualCode])`. The paste future resolves to an
        // `AuthorizationResult` too (`:279`'s `.then`), so the race has one result type.
        enum Winner {
            Redirect(Result<Option<AuthorizationResult>, OAuthError>),
            Manual(Result<String, OAuthError>),
        }
        let winner = tokio::select! {
            settled = &mut waiter => Winner::Redirect(settled),
            prompted = &mut manual => Winner::Manual(prompted),
        };

        let result = match winner {
            Winner::Redirect(settled) => match settled? {
                Some(result) => result,
                // `cancelWait` is never called in this flow, so the listener cannot resolve
                // "manual took over"; a `None` here means the server stopped without settling.
                None => {
                    return Err(OAuthError::Failed(MISSING_CODE_MESSAGE.to_string()));
                }
            },
            Winner::Manual(prompted) => authorization_result_from_manual_input(
                &prompted?,
                state,
                &self.endpoints.redirect_uri,
            )?,
        };

        // `:283`
        interaction.notify(AuthEvent::Progress {
            message: EXCHANGING_MESSAGE.to_string(),
        });
        // `:284`
        self.exchange_authorization_code(
            &result.code,
            verifier,
            &result.client_id,
            interaction.cancel(),
        )
        .await
    }
}

/// Race a future against the login-wide abort. Upstream hands `signal` straight to `fetch`; Rust
/// has no ambient fetch, so the abort is a `select!`. Same shape as
/// [`super::openai_codex`]'s copy.
async fn with_cancel<F: std::future::Future>(
    future: F,
    cancel: Option<&CancelToken>,
) -> Result<F::Output, OAuthError> {
    match cancel {
        Some(token) => {
            if token.is_cancelled() {
                return Err(OAuthError::Cancelled);
            }
            tokio::select! {
                biased;
                () = token.cancelled() => Err(OAuthError::Cancelled),
                output = future => Ok(output),
            }
        }
        None => Ok(future.await),
    }
}

/// `:285-287` — `if (interaction.signal.aborted) throw new Error("Login cancelled")`.
fn map_fetch_error(error: &reqwest::Error, cancel: Option<&CancelToken>) -> OAuthError {
    if cancel.is_some_and(CancelToken::is_cancelled) {
        return OAuthError::Cancelled;
    }
    OAuthError::Failed(error.to_string())
}

#[async_trait::async_trait]
impl OAuthAuth for OpenAiChatGptOAuth {
    /// `:301`.
    fn name(&self) -> &str {
        OPENAI_CHATGPT_OAUTH_NAME
    }

    /// `isSubscription: true` (`:302`) — a ChatGPT Plus/Pro plan, not metered API billing.
    fn is_subscription(&self) -> bool {
        true
    }

    /// `loginLabel: "Sign in with ChatGPT"` (`:303`).
    fn login_label(&self) -> Option<&str> {
        Some(OPENAI_CHATGPT_LOGIN_LABEL)
    }

    /// `loginOpenAIChatGPT` (`:233-298`), including its `finally`.
    async fn login(
        &self,
        interaction: &dyn AuthInteraction,
        options: &LoginOptions,
    ) -> Result<Credential, OAuthError> {
        // `:237` — FIRST, before any listener or authorize URL exists.
        let host_id = agent_host_id(options.device_id().as_deref())?;
        // `:238-240`
        let pkce = generate_pkce()?;
        let state = random_value()?;
        let nonce = random_value()?;
        // `:243-248` — hard-fail; see this module's collision note.
        let server = self.start_callback_server(&state, interaction).await?;
        // `:253` @f1b2e77f5 — `agent_name_hint: options?.agentName ?? AGENT_NAME_HINT` (PROV-144).
        let agent_name_hint = options
            .agent_name
            .as_deref()
            .unwrap_or(self.agent_name_hint.as_str());
        let authorization_url =
            self.authorization_url(agent_name_hint, &host_id, &pkce.challenge, &state, &nonce);

        let manual_abort = CancelToken::new();
        let result = self
            .run_login(
                interaction,
                &server,
                &pkce.verifier,
                &state,
                authorization_url,
                &manual_abort,
            )
            .await;

        // `:288-297` — `finally { manualAbort.abort(); callback.server.close(); … }`.
        manual_abort.cancel();
        server.close();

        // `:285-287` — an abort reports `"Login cancelled"` whatever the inner failure said.
        match result {
            Err(_) if interaction.cancel().is_some_and(CancelToken::is_cancelled) => {
                Err(OAuthError::Cancelled)
            }
            other => other,
        }
    }

    /// `refresh: refreshAccessToken` (`:305`).
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        self.refresh_access_token(cred, None)
            .await
            .map_err(|e| e.into_auth_error(PROVIDER_ID))
    }

    /// `async toAuth(credential) { return { apiKey: credential.access } }` (`:306-308`).
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        match cred {
            Credential::Oauth { access, .. } => Ok(ModelAuth {
                api_key: Some(access.clone()),
                ..Default::default()
            }),
            Credential::ApiKey { .. } => Err(OAuthError::Failed(
                "Sign in with ChatGPT toAuth requires an OAuth credential".to_string(),
            )
            .into_auth_error(PROVIDER_ID)),
        }
    }
}

/// The provider id this flow's failures are attributed to.
pub const PROVIDER_ID: &str = crate::providers::openai::OPENAI_PROVIDER_ID;

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::auth::oauth::interaction::ScriptedInteraction;

    const DEVICE_ID: &str = "e61bbe28-07ef-466d-8e5d-a344f94ab305";

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    // -----------------------------------------------------------------------
    // getDeviceId / agentHostId — `:225-231`
    // -----------------------------------------------------------------------

    /// `:230` — the id is lowercased into a `urn:uuid:` URI.
    #[test]
    fn agent_host_id_is_a_lowercased_uuid_urn() {
        assert_eq!(
            agent_host_id(Some("E61BBE28-07EF-466D-8E5D-A344F94AB305")).unwrap(),
            format!("urn:uuid:{DEVICE_ID}")
        );
    }

    /// Upstream's `"requires a device ID before starting authorization"`: both a missing id and a
    /// non-UUID id fail with `:228`'s message.
    #[test]
    fn a_missing_or_malformed_device_id_is_rejected() {
        for input in [None, Some("not-a-uuid"), Some(""), Some("e61bbe28")] {
            let err = agent_host_id(input).unwrap_err();
            assert_eq!(
                err.to_string(),
                "Sign in with ChatGPT requires a device ID (UUID) for this installation",
                "input {input:?}"
            );
        }
    }

    /// `:237` runs before [`OpenAiChatGptOAuth::start_callback_server`], so a login without a
    /// device id never binds a port and never emits an `auth_url`.
    #[tokio::test]
    async fn login_without_a_device_id_never_starts_authorization() {
        let flow = OpenAiChatGptOAuth::with_endpoints(OpenAiChatGptEndpoints {
            callback_host: Some("127.0.0.1".to_string()),
            callback_port: 0,
            ..Default::default()
        });
        let interaction = ScriptedInteraction::new(Vec::new());
        let err = flow
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Sign in with ChatGPT requires a device ID (UUID) for this installation"
        );
        assert!(
            interaction.events().is_empty(),
            "no authorization should have started: {:?}",
            interaction.events()
        );
        assert!(interaction.prompts().is_empty());
    }

    // -----------------------------------------------------------------------
    // The authorize URL — `:250-263`
    // -----------------------------------------------------------------------

    #[test]
    fn authorize_url_carries_the_dynamic_client_and_the_host_id() {
        let flow = OpenAiChatGptOAuth::new();
        let url = flow.authorization_url(
            AGENT_NAME_HINT,
            &format!("urn:uuid:{DEVICE_ID}"),
            "challenge-value",
            "state-value",
            "nonce-value",
        );
        let parsed = reqwest::Url::parse(&url).unwrap();
        let get = |name: &str| {
            parsed
                .query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        assert_eq!(
            parsed.as_str().split('?').next(),
            Some("https://auth.openai.com/api/accounts/authorize")
        );
        assert_eq!(get("client_id").as_deref(), Some("dynamic_agent_client"));
        assert_eq!(get("agent_name_hint").as_deref(), Some("Pi"));
        assert_eq!(
            get("ext_agent_host_id").as_deref(),
            Some(format!("urn:uuid:{DEVICE_ID}").as_str())
        );
        assert_eq!(get("response_type").as_deref(), Some("code"));
        assert_eq!(
            get("redirect_uri").as_deref(),
            Some("http://127.0.0.1:1455/auth/callback")
        );
        assert_eq!(
            get("resource").as_deref(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            get("scope").as_deref(),
            Some("openid profile email offline_access resource.invoke chatgpt.tokens.use.direct")
        );
        assert_eq!(get("state").as_deref(), Some("state-value"));
        assert_eq!(get("code_challenge").as_deref(), Some("challenge-value"));
        assert_eq!(get("code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(get("nonce").as_deref(), Some("nonce-value"));
    }

    // -----------------------------------------------------------------------
    // The callback contract — `:53-62`
    // -----------------------------------------------------------------------

    #[test]
    fn callback_requires_code_state_and_an_issued_client_id() {
        let ok = authorization_result_from_callback(
            &pairs(&[
                ("code", "authorization-code"),
                ("state", "s"),
                ("client_id", " oaiapp_issued "),
            ]),
            "s",
        )
        .unwrap();
        assert_eq!(ok.code, "authorization-code");
        assert_eq!(ok.client_id, "oaiapp_issued");

        let cases: [(Vec<(String, String)>, &str); 5] = [
            (
                pairs(&[("state", "s"), ("client_id", "c")]),
                "Missing authorization code",
            ),
            (
                pairs(&[("code", "x"), ("client_id", "c")]),
                "Missing OAuth state",
            ),
            (
                pairs(&[("code", "x"), ("state", "other"), ("client_id", "c")]),
                "OAuth state mismatch",
            ),
            (
                pairs(&[("code", "x"), ("state", "s")]),
                "OpenAI OAuth registration callback did not contain an issued client ID",
            ),
            (
                pairs(&[("code", "x"), ("state", "s"), ("client_id", "   ")]),
                "OpenAI OAuth registration callback did not contain an issued client ID",
            ),
        ];
        for (params, expected) in cases {
            let err = authorization_result_from_callback(&params, "s").unwrap_err();
            assert_eq!(err.to_string(), expected, "params {params:?}");
        }
    }

    /// `:64-78` — the pasted URL's origin and path are both checked.
    #[test]
    fn pasted_url_origin_and_path_are_validated() {
        let good = format!("{REDIRECT_URI}?code=c&state=s&client_id=oaiapp_issued");
        assert_eq!(
            authorization_result_from_manual_input(&good, "s", REDIRECT_URI)
                .unwrap()
                .client_id,
            "oaiapp_issued"
        );

        let expected_prefix = format!("The pasted callback URL must start with {REDIRECT_URI}");
        for bad in [
            // different port => different origin
            "http://127.0.0.1:1456/auth/callback?code=c&state=s&client_id=i",
            // different host => different origin
            "http://evil.example/auth/callback?code=c&state=s&client_id=i",
            // different scheme => different origin
            "https://127.0.0.1:1455/auth/callback?code=c&state=s&client_id=i",
            // right origin, wrong path
            "http://127.0.0.1:1455/other?code=c&state=s&client_id=i",
        ] {
            let err = authorization_result_from_manual_input(bad, "s", REDIRECT_URI).unwrap_err();
            assert_eq!(err.to_string(), expected_prefix, "input {bad}");
        }

        assert_eq!(
            authorization_result_from_manual_input("not a url", "s", REDIRECT_URI)
                .unwrap_err()
                .to_string(),
            "Paste the full callback URL from the browser"
        );
        let denied = format!("{REDIRECT_URI}?error=access_denied");
        assert_eq!(
            authorization_result_from_manual_input(&denied, "s", REDIRECT_URI)
                .unwrap_err()
                .to_string(),
            "ChatGPT authorization failed: access_denied"
        );
    }

    // -----------------------------------------------------------------------
    // The token-response contract — `:155-181`
    // -----------------------------------------------------------------------

    #[test]
    fn credential_carries_the_issued_client_id_and_granted_scopes() {
        let token = serde_json::json!({
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "id_token": "id-token",
            "scope": SCOPE,
        });
        let before = now_ms();
        let cred = credential_from_token_response(&token, "oaiapp_issued").unwrap();
        let Credential::Oauth {
            access,
            refresh,
            expires,
            ext,
        } = &cred
        else {
            panic!("expected an oauth credential");
        };
        assert_eq!(access, "access-token");
        assert_eq!(refresh, "refresh-token");
        assert_eq!(
            ext.get(EXT_CLIENT_ID).and_then(serde_json::Value::as_str),
            Some("oaiapp_issued")
        );
        assert_eq!(
            ext.get(EXT_SCOPES)
                .and_then(serde_json::Value::as_array)
                .map(|a| a.len()),
            Some(6)
        );
        // `:177` — three minutes early, so a request never starts with a token about to expire.
        let margin = 3600 * 1000 - EXPIRY_MARGIN_MS;
        assert!(
            *expires >= before.saturating_add(margin) && *expires <= now_ms() + margin,
            "expires {expires} outside [{}, {}]",
            before + margin,
            now_ms() + margin
        );
        assert_eq!(stored_client_id(&cred), Some("oaiapp_issued"));
    }

    #[test]
    fn a_grant_without_direct_token_use_is_refused() {
        let token = serde_json::json!({
            "access_token": "a",
            "refresh_token": "r",
            "expires_in": 3600,
            "id_token": "i",
            "scope": "openid profile email offline_access resource.invoke",
        });
        assert_eq!(
            credential_from_token_response(&token, "c")
                .unwrap_err()
                .to_string(),
            "OpenAI OAuth grant did not include chatgpt.tokens.use.direct"
        );
    }

    /// Upstream's `"requires refresh responses to rotate the refresh token"`.
    #[test]
    fn a_token_response_without_a_rotated_refresh_token_is_refused() {
        let token = serde_json::json!({
            "access_token": "a",
            "expires_in": 3600,
            "id_token": "i",
            "scope": SCOPE,
        });
        assert_eq!(
            credential_from_token_response(&token, "c")
                .unwrap_err()
                .to_string(),
            "OpenAI OAuth token response has invalid refresh_token"
        );
    }

    #[test]
    fn a_non_numeric_expires_in_is_refused() {
        for bad in [
            serde_json::json!("3600"),
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::Value::Null,
        ] {
            let token = serde_json::json!({
                "access_token": "a",
                "refresh_token": "r",
                "expires_in": bad,
                "id_token": "i",
                "scope": SCOPE,
            });
            assert_eq!(
                credential_from_token_response(&token, "c")
                    .unwrap_err()
                    .to_string(),
                "OpenAI OAuth token response has invalid expires_in",
                "expires_in {bad:?}"
            );
        }
    }

    /// `:209-212` — a stored credential with no issued client id cannot be refreshed.
    #[tokio::test]
    async fn refresh_without_a_stored_client_id_names_the_remedy() {
        let flow = OpenAiChatGptOAuth::new();
        let cred = Credential::Oauth {
            refresh: "r".to_string(),
            access: "a".to_string(),
            expires: 0,
            ext: serde_json::Map::new(),
        };
        let err = flow.refresh_access_token(&cred, None).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Stored OpenAI OAuth credential does not contain an issued client ID; reconnect ChatGPT"
        );
    }

    #[tokio::test]
    async fn to_auth_sends_the_access_token_as_the_api_key() {
        let flow = OpenAiChatGptOAuth::new();
        let cred = Credential::Oauth {
            refresh: "r".to_string(),
            access: "chatgpt-access-token".to_string(),
            expires: 0,
            ext: serde_json::Map::new(),
        };
        let auth = flow.to_auth(&cred).await.unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("chatgpt-access-token"));
    }

    // -----------------------------------------------------------------------
    // The strategy's identity — `:300-304`
    // -----------------------------------------------------------------------

    #[test]
    fn strategy_identity_matches_upstream() {
        let flow = OpenAiChatGptOAuth::new();
        assert_eq!(flow.name(), "OpenAI (ChatGPT subscription)");
        assert!(flow.is_subscription());
        assert_eq!(flow.login_label(), Some("Sign in with ChatGPT"));
    }

    // -----------------------------------------------------------------------
    // The port-1455 collision — `:243-248`
    // -----------------------------------------------------------------------

    /// This flow and [`super::super::openai_codex`] bind the SAME port and path, so a login
    /// started while the other holds the port cannot listen.
    #[test]
    fn the_two_openai_logins_share_the_callback_port_and_path() {
        assert_eq!(CALLBACK_PORT, super::super::openai_codex::CALLBACK_PORT);
        assert_eq!(CALLBACK_PATH, super::super::openai_codex::CALLBACK_PATH);
    }

    /// `:243-248` — `EADDRINUSE` fails the login with the exact remedy text, and **no paste prompt
    /// is offered**: v1.0.1 dropped the degrade-to-paste that v1.0.0 had here, which is the
    /// opposite of `PROV-117`. The port is held by a real listener bound in this test, so this
    /// exercises the production bind path rather than a synthesised error.
    #[tokio::test]
    async fn a_contended_port_fails_fast_without_offering_the_paste_fallback() {
        let squatter = std::net::TcpListener::bind("127.0.0.1:0").expect("bind squatter");
        let port = squatter.local_addr().expect("addr").port();
        let flow = OpenAiChatGptOAuth::with_endpoints(OpenAiChatGptEndpoints {
            callback_host: Some("127.0.0.1".to_string()),
            callback_port: port,
            ..Default::default()
        });
        let interaction = ScriptedInteraction::new(Vec::new());
        let err = flow
            .login(
                &interaction,
                &LoginOptions::with_device_id(|| DEVICE_ID.to_string()),
            )
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Port 1455 is in use, probably by an unfinished login in another pi session or by the Codex CLI. Cancel that login and try again."
        );
        // The paste fallback is NOT offered on this path: upstream throws before the
        // `manual_code` prompt at `:272` is ever constructed.
        assert!(
            interaction.prompts().is_empty(),
            "a bind failure must not offer manual paste: {:?}",
            interaction
                .prompts()
                .iter()
                .map(|p| p.kind)
                .collect::<Vec<_>>()
        );
        assert!(
            interaction.events().is_empty(),
            "a bind failure must not emit an auth_url: {:?}",
            interaction.events()
        );
        drop(squatter);
    }

    /// A bind failure that is NOT `EADDRINUSE` propagates unchanged (`:244`'s `throw error`). An
    /// unroutable bind host is the portable way to produce one.
    #[tokio::test]
    async fn a_non_addrinuse_bind_failure_is_not_reworded() {
        let flow = OpenAiChatGptOAuth::with_endpoints(OpenAiChatGptEndpoints {
            callback_host: Some("240.0.0.1".to_string()),
            callback_port: 0,
            ..Default::default()
        });
        let interaction = ScriptedInteraction::new(Vec::new());
        let err = flow
            .login(
                &interaction,
                &LoginOptions::with_device_id(|| DEVICE_ID.to_string()),
            )
            .await
            .unwrap_err();
        assert!(
            !err.to_string().contains("Port 1455 is in use"),
            "a non-EADDRINUSE failure must keep its own message, got: {err}"
        );
        assert!(matches!(err, OAuthError::Listen { .. }), "got {err:?}");
    }

    // -----------------------------------------------------------------------
    // The full login round trip — a translation of upstream's own
    // `test/openai-chatgpt-oauth.test.ts`, with no network and no browser
    // -----------------------------------------------------------------------

    /// Serve canned JSON off `127.0.0.1:0` as the token endpoint, recording each request.
    async fn serve_token(
        status_line: &'static str,
        body: &'static str,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback token endpoint");
        let addr = listener.local_addr().expect("local addr");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = seen.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 16384];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                sink.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).to_string());
                let head = format!(
                    "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (format!("http://{addr}/token"), seen)
    }

    /// Upstream's `loginInteraction` (`test/openai-chatgpt-oauth.test.ts:34-56`): records the
    /// `auth_url` event, then answers the `manual_code` prompt with a callback URL built from that
    /// URL's own `redirect_uri` and `state`. A `ScriptedInteraction` cannot do this — the answer
    /// has to be computed *after* the authorize URL exists.
    struct PasteInteraction {
        /// `options.callbackClientId`: `None` models a registration callback with no issued id.
        callback_client_id: Option<String>,
        authorize_url: std::sync::Mutex<Option<String>>,
    }

    impl PasteInteraction {
        fn new(callback_client_id: Option<&str>) -> Self {
            Self {
                callback_client_id: callback_client_id.map(str::to_string),
                authorize_url: std::sync::Mutex::new(None),
            }
        }
        fn authorize_url(&self) -> Option<String> {
            self.authorize_url.lock().ok().and_then(|u| u.clone())
        }
    }

    #[async_trait::async_trait]
    impl AuthInteraction for PasteInteraction {
        async fn prompt(&self, prompt: AuthPrompt) -> Result<String, OAuthError> {
            if prompt.kind != Some(super::super::interaction::AuthPromptKind::ManualCode) {
                return Err(OAuthError::Failed(format!(
                    "Unexpected prompt: {:?}",
                    prompt.kind
                )));
            }
            let Some(url) = self.authorize_url() else {
                return Err(OAuthError::Failed(
                    "Authorization URL was not emitted before the callback prompt".to_string(),
                ));
            };
            let url = reqwest::Url::parse(&url).expect("authorize url");
            let get = |name: &str| {
                url.query_pairs()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_default()
            };
            let mut callback =
                reqwest::Url::parse(&get("redirect_uri")).expect("redirect_uri is a url");
            {
                let mut q = callback.query_pairs_mut();
                q.append_pair("code", "authorization-code");
                q.append_pair("state", &get("state"));
                if let Some(client_id) = &self.callback_client_id {
                    q.append_pair("client_id", client_id);
                }
            }
            Ok(callback.to_string())
        }

        fn notify(&self, event: AuthEvent) {
            if let AuthEvent::AuthUrl { url, .. } = event
                && let Ok(mut slot) = self.authorize_url.lock()
            {
                *slot = Some(url);
            }
        }
    }

    fn flow_against(token_url: &str) -> OpenAiChatGptOAuth {
        OpenAiChatGptOAuth::with_endpoints(OpenAiChatGptEndpoints {
            token_url: token_url.to_string(),
            // ephemeral, so the listener binds and simply loses the race to the paste
            callback_host: Some("127.0.0.1".to_string()),
            callback_port: 0,
            ..Default::default()
        })
    }

    const TOKEN_BODY: &str = concat!(
        r#"{"access_token":"access-token","refresh_token":"refresh-token","expires_in":3600,"#,
        r#""id_token":"id-token","scope":"openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"}"#
    );

    /// Upstream's `"registers a user-owned client and stores its issued ID and granted scopes"`.
    #[tokio::test]
    async fn login_exchanges_with_the_issued_client_id_and_stores_it() {
        let (token_url, seen) = serve_token("HTTP/1.1 200 OK", TOKEN_BODY).await;
        let flow = flow_against(&token_url);
        let interaction = PasteInteraction::new(Some("oaiapp_issued"));
        let cred = flow
            .login(
                &interaction,
                &LoginOptions::with_device_id(|| DEVICE_ID.to_string()),
            )
            .await
            .expect("login");

        // The authorize URL the user was sent to.
        let url = reqwest::Url::parse(&interaction.authorize_url().expect("auth_url emitted"))
            .expect("url");
        let get = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        assert_eq!(get("client_id").as_deref(), Some("dynamic_agent_client"));
        assert_eq!(
            get("ext_agent_host_id").as_deref(),
            Some(format!("urn:uuid:{DEVICE_ID}").as_str())
        );
        assert_eq!(get("code_challenge_method").as_deref(), Some("S256"));

        // The exchange body — note `client_id` is the ISSUED id read back out of the callback,
        // not `dynamic_agent_client` (`:191`, fed from `:284`'s `result.clientId`).
        let req = seen.lock().unwrap().first().cloned().unwrap_or_default();
        let body = req.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
        let form: Vec<(String, String)> = super::super::query::parse_query(&body);
        let field = |name: &str| {
            form.iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(field("grant_type"), Some("authorization_code"));
        assert_eq!(field("client_id"), Some("oaiapp_issued"));
        assert_eq!(field("code"), Some("authorization-code"));
        assert_eq!(field("redirect_uri"), Some(REDIRECT_URI));
        assert_eq!(field("resource"), Some(RESOURCE));
        assert!(field("code_verifier").is_some_and(|v| !v.is_empty()));

        assert_eq!(stored_client_id(&cred), Some("oaiapp_issued"));
        let Credential::Oauth { access, ext, .. } = &cred else {
            panic!("expected oauth");
        };
        assert_eq!(access, "access-token");
        assert_eq!(
            ext.get(EXT_SCOPES)
                .and_then(serde_json::Value::as_array)
                .and_then(|a| a.last())
                .and_then(serde_json::Value::as_str),
            Some(DIRECT_TOKEN_SCOPE)
        );
    }

    /// PROV-144 — upstream's `"uses the app's agent name as the name hint"`
    /// (`test/openai-chatgpt-oauth.test.ts`, `9ad083102`): `login(…, { getDeviceId, agentName:
    /// "my-app" })` sends `agent_name_hint=my-app` (`openai-chatgpt.ts:253` @f1b2e77f5,
    /// `options?.agentName ?? AGENT_NAME_HINT`); without it the hint stays `Pi`.
    #[tokio::test]
    async fn prov144_uses_the_apps_agent_name_as_the_name_hint() {
        let hint_of = |interaction: &PasteInteraction| {
            let url = reqwest::Url::parse(&interaction.authorize_url().expect("auth_url emitted"))
                .expect("url");
            url.query_pairs()
                .find(|(k, _)| k == "agent_name_hint")
                .map(|(_, v)| v.into_owned())
        };

        let (token_url, _seen) = serve_token("HTTP/1.1 200 OK", TOKEN_BODY).await;
        let interaction = PasteInteraction::new(Some("oaiapp_issued"));
        flow_against(&token_url)
            .login(
                &interaction,
                &LoginOptions::with_device_id(|| DEVICE_ID.to_string()).with_agent_name("my-app"),
            )
            .await
            .expect("login");
        assert_eq!(hint_of(&interaction).as_deref(), Some("my-app"));

        let (token_url, _seen) = serve_token("HTTP/1.1 200 OK", TOKEN_BODY).await;
        let interaction = PasteInteraction::new(Some("oaiapp_issued"));
        flow_against(&token_url)
            .login(
                &interaction,
                &LoginOptions::with_device_id(|| DEVICE_ID.to_string()),
            )
            .await
            .expect("login");
        assert_eq!(hint_of(&interaction).as_deref(), Some("Pi"));
    }

    /// Upstream's `"rejects registration without an issued client ID"`: the callback carried no
    /// `client_id`, so the token endpoint is **never called**. Without the read-back check the
    /// login would exchange against `dynamic_agent_client` and store an unrefreshable credential.
    #[tokio::test]
    async fn a_callback_without_an_issued_client_id_never_reaches_the_token_endpoint() {
        let (token_url, seen) = serve_token("HTTP/1.1 200 OK", TOKEN_BODY).await;
        let flow = flow_against(&token_url);
        let err = flow
            .login(
                &PasteInteraction::new(None),
                &LoginOptions::with_device_id(|| DEVICE_ID.to_string()),
            )
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "OpenAI OAuth registration callback did not contain an issued client ID"
        );
        assert!(
            seen.lock().unwrap().is_empty(),
            "no token request may be made: {:?}",
            seen.lock().unwrap()
        );
    }

    /// Upstream's `"refreshes with the credential's issued client ID and stores replacement
    /// scopes"`: the refresh body carries the stored client id and **no** `scope`.
    #[tokio::test]
    async fn refresh_uses_the_stored_client_id_and_sends_no_scope() {
        let (token_url, seen) = serve_token("HTTP/1.1 200 OK", TOKEN_BODY).await;
        let flow = flow_against(&token_url);
        let mut ext = serde_json::Map::new();
        ext.insert(
            EXT_CLIENT_ID.to_string(),
            serde_json::Value::String("oaiapp_existing".to_string()),
        );
        let cred = Credential::Oauth {
            refresh: "old-refresh".to_string(),
            access: "old-access".to_string(),
            expires: 0,
            ext,
        };
        let refreshed = flow
            .refresh_access_token(&cred, None)
            .await
            .expect("refresh");

        let req = seen.lock().unwrap().first().cloned().unwrap_or_default();
        let body = req.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
        let form: Vec<(String, String)> = super::super::query::parse_query(&body);
        let field = |name: &str| {
            form.iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(field("grant_type"), Some("refresh_token"));
        assert_eq!(field("client_id"), Some("oaiapp_existing"));
        assert_eq!(field("refresh_token"), Some("old-refresh"));
        assert_eq!(field("resource"), Some(RESOURCE));
        assert_eq!(field("scope"), None, "a refresh must not re-request scopes");

        let Credential::Oauth {
            access,
            refresh,
            expires,
            ..
        } = &refreshed
        else {
            panic!("expected oauth");
        };
        assert_eq!(access, "access-token");
        assert_eq!(refresh, "refresh-token");
        // `:177` — the three-minute margin is applied on refresh too.
        assert!(
            *expires <= now_ms() + 3600 * 1000 - EXPIRY_MARGIN_MS,
            "refresh must keep the {EXPIRY_MARGIN_MS}ms margin"
        );
        assert_eq!(stored_client_id(&refreshed), Some("oaiapp_existing"));
    }

    /// A non-2xx token reply is upstream's `:146` message. The body of a *failed* token call is an
    /// OAuth error code, never a granted token, so echoing it is not a leak.
    #[tokio::test]
    async fn a_failed_token_request_reports_the_status_and_body() {
        let (token_url, _seen) =
            serve_token("HTTP/1.1 400 Bad Request", r#"{"error":"invalid_grant"}"#).await;
        let flow = flow_against(&token_url);
        let err = flow
            .login(
                &PasteInteraction::new(Some("oaiapp_issued")),
                &LoginOptions::with_device_id(|| DEVICE_ID.to_string()),
            )
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            r#"OpenAI OAuth token request failed (400): {"error":"invalid_grant"}"#
        );
    }

    // -----------------------------------------------------------------------
    // Secrets discipline
    // -----------------------------------------------------------------------

    /// No token material may reach a log, an error or a `Debug` rendering. The two types this
    /// module introduces that travel near a credential are [`OpenAiChatGptEndpoints`] (URLs only)
    /// and [`AuthorizationResult`]; the stored credential's own `Debug` is
    /// [`Credential`]'s. This pins all three at once.
    #[test]
    fn nothing_near_a_credential_renders_its_secret() {
        let cred = Credential::Oauth {
            refresh: "refresh-SECRET".to_string(),
            access: "access-SECRET".to_string(),
            expires: 0,
            ext: serde_json::Map::new(),
        };
        let rendered = format!("{cred:?}");
        assert!(
            !rendered.contains("SECRET"),
            "Credential Debug leaked token material: {rendered}"
        );

        assert!(
            rendered.contains("Credential::Oauth") && rendered.contains("<redacted>"),
            "the redacted render should still say what it is: {rendered}"
        );

        let endpoints = format!("{:?}", OpenAiChatGptEndpoints::default());
        assert!(!endpoints.contains("SECRET"));

        // The authorization code is exchangeable credential material.
        let rendered = format!(
            "{:?}",
            AuthorizationResult {
                code: "code-SECRET".to_string(),
                client_id: "oaiapp_issued".to_string(),
            }
        );
        assert!(
            !rendered.contains("SECRET"),
            "AuthorizationResult Debug leaked the authorization code: {rendered}"
        );
        assert!(rendered.contains("oaiapp_issued"));

        // The device id is an installation identifier the app owns; it must not leak through the
        // failure path either.
        let err = agent_host_id(Some("00000000-0000-4000-8000-00000000SECRET")).unwrap_err();
        assert!(
            !err.to_string().contains("SECRET"),
            "device id leaked into the error: {err}"
        );

        // The `authorization_code` never appears in a rejection message.
        let err = authorization_result_from_manual_input(
            &format!("{REDIRECT_URI}?code=code-SECRET&state=other&client_id=i"),
            "expected",
            REDIRECT_URI,
        )
        .unwrap_err();
        assert!(
            !err.to_string().contains("SECRET"),
            "authorization code leaked into the error: {err}"
        );
    }
}
