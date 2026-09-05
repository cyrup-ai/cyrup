//! OpenAI Codex (ChatGPT OAuth) **login** flow — port of pi v0.83.0
//! `packages/ai/src/auth/oauth/openai-codex.ts` (538 lines), the largest of upstream's flows.
//!
//! Upstream's header: "OpenAI Codex (ChatGPT OAuth) flow. NOTE: This module uses Node.js crypto
//! and http for the OAuth callback. It is only intended for CLI use, not browser environments."
//! (`openai-codex.ts:1-5`).
//!
//! This is the ChatGPT Plus/Pro subscription login. It offers **two** methods behind a `select`
//! prompt (`:394-410`): a PKCE redirect flow against a fixed `localhost:1455/auth/callback`
//! listener, raced against a manual paste; and an OpenAI-specific *device* flow that is **not**
//! RFC 8628 — it exchanges a user code for an ordinary `authorization_code` + `code_verifier`
//! pair, which is then run through the same token endpoint with a different `redirect_uri`.
//!
//! ## Provenance
//!
//! | this module | `openai-codex.ts` |
//! |---|---|
//! | [`CLIENT_ID`]/[`AUTH_BASE_URL`]/[`AUTHORIZE_URL`]/[`TOKEN_URL`]/[`REDIRECT_URI`] | `:26-30` |
//! | [`DEVICE_USER_CODE_URL`]/[`DEVICE_TOKEN_URL`]/[`DEVICE_VERIFICATION_URI`]/[`DEVICE_REDIRECT_URI`] | `:31-34` |
//! | [`DEVICE_CODE_TIMEOUT_SECONDS`]/[`BROWSER_LOGIN_METHOD`]/[`DEVICE_CODE_LOGIN_METHOD`]/[`SCOPE`]/[`JWT_CLAIM_PATH`] | `:35-39` |
//! | [`OAuthToken`] | `type OAuthToken`, `:41` |
//! | [`OpenAiCodexOAuthFlow::bind_host`] | `getCallbackHost`, `:44-46` |
//! | [`DeviceAuthInfo`] | `DeviceAuthInfo`, `:48-52` |
//! | [`DeviceTokenSuccess`] | `DeviceTokenSuccess`, `:54-57` |
//! | [`create_state`] | `createState`, `:65-70` |
//! | [`parse_authorization_input`] | `parseAuthorizationInput`, `:72-98` |
//! | [`OpenAiCodexOAuthFlow::read_token_response`] | `readTokenResponse`, `:122-146` |
//! | [`OpenAiCodexOAuthFlow::exchange_authorization_code`] | `exchangeAuthorizationCode`, `:148-158` |
//! | [`OpenAiCodexOAuthFlow::start_device_auth`] | `startOpenAICodexDeviceAuth`, `:181-219` |
//! | [`DeviceTokenPoller`] | the `poll` closure of `pollOpenAICodexDeviceAuth`, `:221-274` |
//! | [`OpenAiCodexOAuthFlow::create_authorization_flow`] | `createAuthorizationFlow`, `:276-295` |
//! | [`OpenAiCodexOAuthFlow::start_local_oauth_server`] + [`CodexCallbackHandler`] | `startLocalOAuthServer`, `:303-375` |
//! | [`credentials_from_token`] | `credentialsFromToken`, `:384-397` |
//! | [`OpenAiCodexOAuthFlow::login_device_code`] | `loginOpenAICodexDeviceCode`, `:408-424` |
//! | [`OpenAiCodexOAuthFlow::login_browser`] / [`OpenAiCodexOAuthFlow::run_login_browser`] | `loginOpenAICodex`, `:426-486` |
//! | `impl OAuthAuth for OpenAiCodexOAuthFlow` | `openaiCodexOAuth`, `:492-538` |
//!
//! `getAccountId` (`:377-382`) and `decodeJwt` (`:100-112`) were ported ahead of this module by
//! the provider (`providers/openai_codex.rs`, commit `2b91630`) because a *stored* credential
//! needs them on every refresh; this module calls
//! [`crate::providers::openai_codex::openai_codex_account_id`] rather than porting them twice.
//! `refreshAccessToken`/`refreshOpenAICodexToken` (`:160-179`, `:488-490`) live there for the same
//! reason, so [`OAuthAuth::refresh`] and [`OAuthAuth::to_auth`] here delegate to
//! [`crate::providers::openai_codex::OpenAiCodexOAuth`] instead of re-implementing them — one
//! behaviour, one implementation.
//!
//! `parseAuthorizationInput` (`:72-98`) is byte-identical to `anthropic.ts:52-79`; upstream keeps
//! two copies, this crate keeps one ([`super::anthropic::parse_authorization_input`]) and pins the
//! behaviour `openai-codex.ts` depends on with its own tests below, so a change made for Anthropic
//! cannot silently change Codex.
//!
//! ## Mechanism divergences (Rust forces these; behaviour is unchanged unless noted)
//!
//! * **Callback server.** Upstream calls `node:http.createServer` (`:11-19`, `:315`); this crate's
//!   `tokio` has no `net` feature, so the listener is [`super::callback::CallbackServer`]. Two
//!   consequences are visible only to a client that is not a browser following a redirect:
//!   1. The shared server answers a foreign route with `"OAuth callback route not found."`
//!      (`callback.rs`), where `:318-322` says `"Callback route not found."`. The status (404) and
//!      the page are otherwise identical. Fixing it means changing the shared server, which is
//!      outside this module; see this change's `not_done`.
//!   2. `:334-338`'s 500 `"Internal error while processing OAuth callback."` has no counterpart: a
//!      Rust handler returns a [`CallbackOutcome`] and cannot throw, so the `catch` is unreachable
//!      rather than omitted.
//!   3. `:361-374` — a bind failure resolves `waitForCode()` with `null` instead of rejecting, so
//!      the login continues and can still be completed by pasting the redirect URL. That is
//!      reproduced by mapping [`OAuthError::Listen`] to `None`; every other start failure (an
//!      already-aborted login) still propagates.
//! * **No ambient `fetch`.** Requests go through `reqwest`. `AbortSignal` is a [`CancelToken`]
//!   (arch-00 §3.2) raced against the request rather than handed to it, so
//!   `fetchWithLoginCancellation` (`:114-120`) — "an aborted signal reports `Login cancelled`,
//!   whatever the transport said" — is [`with_cancel`] plus [`map_fetch_error`].
//! * **`await response.json()`** throws upstream when the body is not JSON, and that rejection
//!   propagates raw. Rust reports it through the same `"… response missing fields: null"` /
//!   `"Invalid … response: null"` message the field check would produce, matching
//!   `providers/openai_codex.rs`'s existing port of `readTokenResponse`: from the caller's side
//!   both mean "the endpoint did not answer with a token".
//! * **Endpoint override.** Upstream's tests stub the ambient `fetch`; Rust has none, so every
//!   endpoint is a field of [`OpenAiCodexEndpoints`], defaulting to the upstream constants.
//!   Production callers use [`OpenAiCodexOAuthFlow::new`].
//! * **`JSON.stringify` key order** inside the three "here is what you sent me" messages — see
//!   [`json_text`].
//! * **`Number(...)` on a string `interval`** (`:206`) is [`js_number`], which reproduces the
//!   decimal/exponent/`Infinity`/empty-string cases; JS's legacy `0x`/`0b` literal forms are not
//!   accepted. No authorization server sends a hexadecimal poll interval, and both spellings fail
//!   the `Number.isFinite` guard the same way when they are not numbers.

use super::callback::{
    CallbackControl, CallbackHandler, CallbackOutcome, CallbackReply, CallbackRequest,
    CallbackServer, CallbackServerConfig, callback_host,
};
use super::device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, DeviceCodePoller, poll_oauth_device_code_flow,
};
use super::interaction::{AuthEvent, AuthInteraction, AuthPrompt, AuthSelectOption};
use super::pkce::generate_pkce;
use super::query::encode_query;
use super::random::random_bytes;
use super::{OAuthError, now_ms};
use crate::auth::OAuthAuth;
use crate::auth::types::{Credential, EnvAuthContext, ModelAuth};
use crate::error::AuthError;
use crate::providers::openai_codex::{
    EXT_ACCOUNT_ID, OPENAI_CODEX_OAUTH_NAME, OPENAI_CODEX_PROVIDER_ID, openai_codex_account_id,
};
use cyrup_core::CancelToken;

// ---------------------------------------------------------------------------
// Constants — openai-codex.ts:26-39
// ---------------------------------------------------------------------------

/// `CLIENT_ID` (`openai-codex.ts:26`). Public OAuth client id; identical to
/// [`crate::providers::openai_codex::OPENAI_CODEX_CLIENT_ID`], re-stated here because upstream
/// states it here.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// `AUTH_BASE_URL` (`openai-codex.ts:27`).
pub const AUTH_BASE_URL: &str = "https://auth.openai.com";
/// `AUTHORIZE_URL` (`openai-codex.ts:28`).
pub const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
/// `TOKEN_URL` (`openai-codex.ts:29`).
pub const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// `REDIRECT_URI` (`openai-codex.ts:30`) — a **registered** redirect, so the browser flow cannot
/// use an ephemeral port the way `openrouter.ts` does.
pub const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
/// `DEVICE_USER_CODE_URL` (`openai-codex.ts:31`).
pub const DEVICE_USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
/// `DEVICE_TOKEN_URL` (`openai-codex.ts:32`).
pub const DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
/// `DEVICE_VERIFICATION_URI` (`openai-codex.ts:33`) — the page the user opens.
pub const DEVICE_VERIFICATION_URI: &str = "https://auth.openai.com/codex/device";
/// `DEVICE_REDIRECT_URI` (`openai-codex.ts:34`) — the `redirect_uri` the device flow's
/// authorization code was minted against, and therefore the one its exchange must present.
pub const DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
/// `DEVICE_CODE_TIMEOUT_SECONDS = 15 * 60` (`openai-codex.ts:35`).
pub const DEVICE_CODE_TIMEOUT_SECONDS: f64 = 15.0 * 60.0;
/// `OPENAI_CODEX_BROWSER_LOGIN_METHOD` (`openai-codex.ts:36`).
pub const BROWSER_LOGIN_METHOD: &str = "browser";
/// `OPENAI_CODEX_DEVICE_CODE_LOGIN_METHOD` (`openai-codex.ts:37`).
pub const DEVICE_CODE_LOGIN_METHOD: &str = "device_code";
/// `SCOPE` (`openai-codex.ts:38`). Space-separated; the urlencoded serializer turns the spaces
/// into `+`.
pub const SCOPE: &str = "openid profile email offline_access";
/// `JWT_CLAIM_PATH` (`openai-codex.ts:39`).
pub const JWT_CLAIM_PATH: &str = "https://api.openai.com/auth";

/// `originator` (`openai-codex.ts:277`, `:292`) — an upstream-registered client identifier paired
/// with [`CLIENT_ID`], not a cyrup-facing brand string, so rebranding it would change what
/// OpenAI's authorization server sees. Compare `xai.rs`'s `REFERRER`.
pub const ORIGINATOR: &str = "pi";

/// The callback path of [`REDIRECT_URI`] (`openai-codex.ts:319`).
pub const CALLBACK_PATH: &str = "/auth/callback";
/// The callback port of [`REDIRECT_URI`] (`openai-codex.ts:363`).
pub const CALLBACK_PORT: u16 = 1455;
/// The host [`REDIRECT_URI`] advertises. The listener *binds* [`callback_host`]'s value (default
/// `127.0.0.1`), but the registered redirect names `localhost` (`openai-codex.ts:30`, `:363`).
pub const ADVERTISE_HOST: &str = "localhost";

/// `openai-codex.ts:396`.
const NO_ACCOUNT_ID_MESSAGE: &str = "Failed to extract accountId from token";
/// `openai-codex.ts:484`.
const MISSING_CODE_MESSAGE: &str = "Missing authorization code";
/// `openai-codex.ts:445`, `:471`, `:479` — note this is `"State mismatch"`, **not** Anthropic's
/// `"OAuth state mismatch"`.
const STATE_MISMATCH_MESSAGE: &str = "State mismatch";
/// `openai-codex.ts:414`.
const AUTH_URL_INSTRUCTIONS: &str = "A browser window should open. Complete login to finish.";
/// `openai-codex.ts:452`.
const MANUAL_PROMPT_MESSAGE: &str =
    "Complete login in your browser, or paste the authorization code / redirect URL here:";
/// `openai-codex.ts:190-192`.
const DEVICE_NOT_ENABLED_MESSAGE: &str = "OpenAI Codex device code login is not enabled for this server. Use browser login or verify the server URL.";
/// `openai-codex.ts:496`.
const SELECT_LOGIN_METHOD_MESSAGE: &str = "Select OpenAI Codex login method:";
/// `openai-codex.ts:498`.
const BROWSER_LOGIN_LABEL: &str = "Browser login (default)";
/// `openai-codex.ts:499`.
const DEVICE_CODE_LOGIN_LABEL: &str = "Device code login (headless)";

/// JS truthiness for an optional string: `undefined`/`null` and `""` are both falsy. Upstream
/// depends on it at `:429`, `:445`, `:470` and `:484` — `searchParams.get()` yields `""` for a
/// bare `?code=`, which has to read as missing.
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|s| !s.is_empty())
}

/// Lowercase hex, the encoding of `randomBytes(16).toString("hex")` (`openai-codex.ts:69`).
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

/// `createState` (`openai-codex.ts:65-70`): 16 random bytes as 32 lowercase hex characters.
///
/// Upstream's `if (!_randomBytes) throw "OpenAI Codex OAuth is only available in Node.js
/// environments"` (`:66-68`) is the browser-bundle guard; the Rust analogue is
/// [`OAuthError::Entropy`], raised by [`random_bytes`] when no OS CSPRNG is reachable.
pub fn create_state() -> Result<String, OAuthError> {
    Ok(hex(&random_bytes(16)?))
}

/// `Number(value)` for the string form of `interval` (`openai-codex.ts:206`).
///
/// Reproduces the cases a JSON payload can carry: an empty/whitespace-only string is `0`, a
/// decimal or exponent literal parses, `Infinity` parses, anything else is `NaN` (here `None`).
/// See this module's divergence note for the legacy `0x`/`0b` forms.
fn js_number(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Some(0.0);
    }
    trimmed.parse::<f64>().ok()
}

/// `JSON.stringify(value)` for the message templates at `:139`, `:212` and `:245`. `undefined`
/// stringifies to the literal `undefined` in a template, but every call site here passes a parsed
/// (possibly `null`) JSON value, so `null` is the degenerate case.
///
/// **Divergence, message-only.** `JSON.stringify` emits object keys in insertion order;
/// `serde_json::Value` stores them in a `BTreeMap` unless the crate-wide `preserve_order` feature
/// is on, so keys come out **sorted**. The prefix, the payload and the failure itself are
/// upstream's; only the key order inside the echoed payload can differ. Turning it on is a
/// workspace-manifest decision outside this module —
/// [`crate::providers::openai_codex`]'s already-shipped port of `readTokenResponse` renders the
/// same way, so the two halves of this flow at least agree. See this change's `not_done`.
fn json_text(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

// ---------------------------------------------------------------------------
// Types — openai-codex.ts:41, :48-57
// ---------------------------------------------------------------------------

/// `type OAuthToken = { access; refresh; expires }` (`openai-codex.ts:41`). `expires` is Unix
/// **milliseconds**, already stamped `Date.now() + expires_in * 1000` (`:144`) with **no** skew —
/// unlike `anthropic.ts:225`, which subtracts five minutes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthToken {
    pub access: String,
    pub refresh: String,
    pub expires: i64,
}

/// `DeviceAuthInfo` (`openai-codex.ts:48-52`).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceAuthInfo {
    pub device_auth_id: String,
    pub user_code: String,
    pub interval_seconds: f64,
}

/// `DeviceTokenSuccess` (`openai-codex.ts:54-57`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceTokenSuccess {
    pub authorization_code: String,
    pub code_verifier: String,
}

/// `{ verifier, state, url }` (`createAuthorizationFlow`, `openai-codex.ts:276-295`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationFlow {
    pub verifier: String,
    pub state: String,
    pub url: String,
}

// ---------------------------------------------------------------------------
// parseAuthorizationInput — openai-codex.ts:72-98
// ---------------------------------------------------------------------------

/// `parseAuthorizationInput` (`openai-codex.ts:72-98`).
///
/// Byte-identical to `anthropic.ts:52-79`, so it is [`super::anthropic::parse_authorization_input`]
/// rather than a second copy. The tests in this module assert the four branches
/// `openai-codex.ts` depends on directly, so a change made for Anthropic cannot silently change
/// this flow.
pub use super::anthropic::{ParsedAuthorizationInput, parse_authorization_input};

// ---------------------------------------------------------------------------
// credentialsFromToken — openai-codex.ts:384-397
// ---------------------------------------------------------------------------

/// `credentialsFromToken` (`openai-codex.ts:384-397`): stamp the ChatGPT account id, read out of
/// the access token's JWT payload, onto the stored credential. A token with no usable
/// `chatgpt_account_id` claim fails with upstream's exact string.
pub fn credentials_from_token(token: OAuthToken) -> Result<Credential, OAuthError> {
    // `:385-389` — `getAccountId` is `providers/openai_codex.rs`'s port; see the module note.
    let account_id = openai_codex_account_id(&token.access)
        .ok_or_else(|| OAuthError::Failed(NO_ACCOUNT_ID_MESSAGE.to_string()))?;
    let mut ext = serde_json::Map::new();
    ext.insert(
        EXT_ACCOUNT_ID.to_string(),
        serde_json::Value::String(account_id),
    );
    // `:391-396`
    Ok(Credential::Oauth {
        refresh: token.refresh,
        access: token.access,
        expires: token.expires,
        ext,
    })
}

// ---------------------------------------------------------------------------
// The callback handler — openai-codex.ts:315-345
// ---------------------------------------------------------------------------

/// The Codex half of the callback server (`openai-codex.ts:315-345`).
///
/// Every rejection branch replies **without** settling the wait, exactly as upstream `res.end()`s
/// without reaching `settleWait`: the browser sees the error page and the listener keeps waiting.
pub struct CodexCallbackHandler {
    /// `state` (`openai-codex.ts:303`), the value [`create_state`] produced for this login.
    expected_state: String,
}

#[async_trait::async_trait]
impl CallbackHandler for CodexCallbackHandler {
    type Value = String;

    async fn handle(
        &self,
        request: CallbackRequest,
        _control: CallbackControl,
    ) -> CallbackOutcome<String> {
        // `:318-322` — the route check is the shared server's; see the module divergence note.
        // `:323-328`
        if request.param("state") != Some(self.expected_state.as_str()) {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(400, "State mismatch.", None),
            };
        }
        // `:329-335`
        let code = request.param("code");
        if !truthy(code) {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(400, "Missing authorization code.", None),
            };
        }
        // `:336-339`
        CallbackOutcome::Complete {
            reply: CallbackReply::success(
                "OpenAI authentication completed. You can close this window.",
            ),
            value: code.unwrap_or_default().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// The device-code poller — openai-codex.ts:221-274
// ---------------------------------------------------------------------------

/// The `poll` closure of `pollOpenAICodexDeviceAuth` (`openai-codex.ts:226-273`).
struct DeviceTokenPoller<'a> {
    flow: &'a OpenAiCodexOAuthFlow,
    device: DeviceAuthInfo,
    cancel: Option<CancelToken>,
}

#[async_trait::async_trait]
impl DeviceCodePoller for DeviceTokenPoller<'_> {
    type Value = DeviceTokenSuccess;

    async fn poll(&self) -> Result<DeviceCodePollResult<DeviceTokenSuccess>, OAuthError> {
        let cancel = self.cancel.as_ref();
        let client = self
            .flow
            .client(&self.flow.endpoints.device_token_url)
            .await?;
        // `:227-236`
        let body = serde_json::json!({
            "device_auth_id": self.device.device_auth_id,
            "user_code": self.device.user_code,
        });
        let send = client
            .post(&self.flow.endpoints.device_token_url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
        let response = match with_cancel(send, cancel).await? {
            Ok(response) => response,
            Err(error) => return Err(map_fetch_error(&error, cancel)),
        };

        let status = response.status();
        let text = match with_cancel(response.text(), cancel).await? {
            Ok(text) => text,
            // `.catch(() => "")` (`:250`) only guards the non-ok read; an ok body that cannot be
            // read is a rejected `response.json()` (`:239`), which propagates.
            Err(error) if status.is_success() => return Err(map_fetch_error(&error, cancel)),
            Err(_) => String::new(),
        };

        // `:238-248`
        if status.is_success() {
            let json: serde_json::Value =
                serde_json::from_str(text.trim()).unwrap_or(serde_json::Value::Null);
            let authorization_code = json
                .get("authorization_code")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty());
            let code_verifier = json
                .get("code_verifier")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty());
            return Ok(match (authorization_code, code_verifier) {
                (Some(authorization_code), Some(code_verifier)) => {
                    DeviceCodePollResult::Complete(DeviceTokenSuccess {
                        authorization_code: authorization_code.to_string(),
                        code_verifier: code_verifier.to_string(),
                    })
                }
                // `:241-245`
                _ => DeviceCodePollResult::Failed {
                    message: format!(
                        "Invalid OpenAI Codex device auth token response: {}",
                        json_text(&json)
                    ),
                },
            });
        }

        // `:252-254` — the two statuses OpenAI answers an unfinished authorization with.
        if status.as_u16() == 403 || status.as_u16() == 404 {
            return Ok(DeviceCodePollResult::Pending);
        }

        // `:256-263` — `json?.error` is read only when the body parses to an object; a nested
        // `{ code }` object contributes its `code`, and only a *string* can match below.
        let error_code = serde_json::from_str::<serde_json::Value>(text.trim())
            .ok()
            .and_then(|json| json.get("error").cloned())
            .and_then(|error| match error {
                serde_json::Value::Object(map) => map.get("code").cloned(),
                other => Some(other),
            })
            .and_then(|value| value.as_str().map(str::to_string));

        // `:265-270`
        match error_code.as_deref() {
            Some("deviceauth_authorization_pending") => Ok(DeviceCodePollResult::Pending),
            // `:269` sends no new interval, so RFC 8628 §3.5's +5s applies.
            Some("slow_down") => Ok(DeviceCodePollResult::SlowDown {
                interval_seconds: None,
            }),
            // `:271-274`
            _ => Ok(DeviceCodePollResult::Failed {
                message: format!(
                    "OpenAI Codex device auth failed with status {}{}",
                    status.as_u16(),
                    body_suffix(&text)
                ),
            }),
        }
    }
}

/// `${responseBody ? `: ${responseBody}` : ""}` (`openai-codex.ts:197`, `:272`) — JS truthiness,
/// so an empty body contributes nothing.
fn body_suffix(body: &str) -> String {
    if body.is_empty() {
        String::new()
    } else {
        format!(": {body}")
    }
}

// ---------------------------------------------------------------------------
// openaiCodexOAuth — openai-codex.ts:492-538
// ---------------------------------------------------------------------------

/// Every endpoint the flow talks to, so a test can point them at loopback listeners.
///
/// Exists only because Rust has no ambient `fetch` for a test to stub the way upstream's would;
/// [`Default`] is upstream's `:26-34` constants verbatim and is what
/// [`OpenAiCodexOAuthFlow::new`] uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenAiCodexEndpoints {
    pub authorize_url: String,
    pub token_url: String,
    pub device_user_code_url: String,
    pub device_token_url: String,
    pub device_verification_uri: String,
    pub device_redirect_uri: String,
    /// The **registered** redirect (`openai-codex.ts:30`), sent in the authorize URL and in the
    /// browser flow's token exchange. Upstream uses the constant regardless of what the listener
    /// actually bound, and so does this port.
    pub redirect_uri: String,
    /// `None` resolves `CYRUP_OAUTH_CALLBACK_HOST`/`PI_OAUTH_CALLBACK_HOST` at login time, which
    /// is what `getCallbackHost()` does per login (`openai-codex.ts:44-46`, `:363`).
    pub callback_host: Option<String>,
    /// `0` binds an ephemeral port; production is [`CALLBACK_PORT`].
    pub callback_port: u16,
}

impl Default for OpenAiCodexEndpoints {
    fn default() -> Self {
        Self {
            authorize_url: AUTHORIZE_URL.to_string(),
            token_url: TOKEN_URL.to_string(),
            device_user_code_url: DEVICE_USER_CODE_URL.to_string(),
            device_token_url: DEVICE_TOKEN_URL.to_string(),
            device_verification_uri: DEVICE_VERIFICATION_URI.to_string(),
            device_redirect_uri: DEVICE_REDIRECT_URI.to_string(),
            redirect_uri: REDIRECT_URI.to_string(),
            callback_host: None,
            callback_port: CALLBACK_PORT,
        }
    }
}

/// The OpenAI Codex (ChatGPT Plus/Pro) OAuth strategy — upstream's `openaiCodexOAuth`
/// (`openai-codex.ts:492-538`), complete with `login`.
pub struct OpenAiCodexOAuthFlow {
    endpoints: OpenAiCodexEndpoints,
    originator: String,
    /// `refresh`/`toAuth` (`openai-codex.ts:534-537`) were ported with the provider; delegating
    /// keeps one implementation of the refresh wire call rather than two.
    runtime: crate::providers::openai_codex::OpenAiCodexOAuth,
}

impl Default for OpenAiCodexOAuthFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenAiCodexOAuthFlow {
    /// The production strategy — upstream's `:26-34` endpoints.
    pub fn new() -> Self {
        Self::with_endpoints(OpenAiCodexEndpoints::default())
    }

    /// Point the flow at different endpoints. Test-facing seam; production code uses
    /// [`OpenAiCodexOAuthFlow::new`].
    pub fn with_endpoints(endpoints: OpenAiCodexEndpoints) -> Self {
        Self {
            endpoints,
            originator: ORIGINATOR.to_string(),
            runtime: crate::providers::openai_codex::OpenAiCodexOAuth::new(),
        }
    }

    /// The endpoints in force, for callers that need to build a matching authorize URL.
    pub fn endpoints(&self) -> &OpenAiCodexEndpoints {
        &self.endpoints
    }

    /// PROV-047: proxy-aware, per target. `build_client()` consulted neither the ported resolver
    /// nor the `httpProxy` setting, so every OAuth token exchange and silent refresh bypassed a
    /// configured proxy while provider streaming used it.
    async fn client(&self, target_url: &str) -> Result<reqwest::Client, OAuthError> {
        crate::stream::sse::build_client_for(target_url)
            .await
            .map_err(|e| OAuthError::Failed(e.to_string()))
    }

    /// `getCallbackHost()` (`openai-codex.ts:44-46`).
    async fn bind_host(&self) -> String {
        match &self.endpoints.callback_host {
            Some(host) => host.clone(),
            None => callback_host(&EnvAuthContext, None).await,
        }
    }

    /// `createAuthorizationFlow` (`openai-codex.ts:276-295`). Parameter order is upstream's
    /// insertion order, which `URLSearchParams.toString()` preserves.
    pub fn create_authorization_flow(&self) -> Result<AuthorizationFlow, OAuthError> {
        // `:279-280`
        let pkce = generate_pkce()?;
        let state = create_state()?;

        // `:282-293`
        let params = encode_query([
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", self.endpoints.redirect_uri.as_str()),
            ("scope", SCOPE),
            ("code_challenge", pkce.challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", self.originator.as_str()),
        ]);

        Ok(AuthorizationFlow {
            verifier: pkce.verifier,
            state,
            url: format!("{}?{params}", self.endpoints.authorize_url),
        })
    }

    /// `readTokenResponse` (`openai-codex.ts:122-146`). `operation` is upstream's word — the
    /// literal `exchange` or `refresh` that distinguishes the two failure strings.
    async fn read_token_response(
        response: reqwest::Response,
        operation: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<OAuthToken, OAuthError> {
        let status = response.status();
        // `:123-126`
        if !status.is_success() {
            let text = with_cancel(response.text(), cancel)
                .await?
                .unwrap_or_default();
            // `${text || response.statusText}` (`:125`) — JS truthiness, so an empty body falls
            // back to the reason phrase.
            let detail = if text.is_empty() {
                status.canonical_reason().unwrap_or_default().to_string()
            } else {
                text
            };
            return Err(OAuthError::Failed(format!(
                "OpenAI Codex token {operation} failed ({}): {detail}",
                status.as_u16()
            )));
        }

        // `:128-137` — see the module note on a body that is not JSON.
        let text = match with_cancel(response.text(), cancel).await? {
            Ok(text) => text,
            Err(error) => return Err(map_fetch_error(&error, cancel)),
        };
        let json: serde_json::Value =
            serde_json::from_str(text.trim()).unwrap_or(serde_json::Value::Null);

        // `!json?.access_token || !json.refresh_token || typeof json.expires_in !== "number"`
        // (`:136`). JS truthiness makes `""` missing; a numeric *string* `expires_in` fails.
        let access = json
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty());
        let refresh = json
            .get("refresh_token")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty());
        let expires_in = json.get("expires_in").and_then(serde_json::Value::as_f64);

        match (access, refresh, expires_in) {
            // `:141-145` — no skew, unlike `anthropic.ts:225`.
            (Some(access), Some(refresh), Some(expires_in)) => Ok(OAuthToken {
                access: access.to_string(),
                refresh: refresh.to_string(),
                expires: now_ms().saturating_add((expires_in * 1000.0) as i64),
            }),
            // `:137-139`
            _ => Err(OAuthError::Failed(format!(
                "OpenAI Codex token {operation} response missing fields: {}",
                json_text(&json)
            ))),
        }
    }

    /// `exchangeAuthorizationCode` (`openai-codex.ts:148-158`) — form-encoded, in upstream's
    /// field order.
    pub async fn exchange_authorization_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<OAuthToken, OAuthError> {
        let client = self.client(&self.endpoints.token_url).await?;
        // `:152-157`
        let body = encode_query([
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
        ]);
        let send = client
            .post(&self.endpoints.token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send();
        let response = match with_cancel(send, cancel).await? {
            Ok(response) => response,
            Err(error) => return Err(map_fetch_error(&error, cancel)),
        };
        Self::read_token_response(response, "exchange", cancel).await
    }

    /// `exchangeAuthorizationCodeForCredentials` (`openai-codex.ts:399-406`).
    async fn exchange_authorization_code_for_credentials(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<Credential, OAuthError> {
        credentials_from_token(
            self.exchange_authorization_code(code, verifier, redirect_uri, cancel)
                .await?,
        )
    }

    /// `startOpenAICodexDeviceAuth` (`openai-codex.ts:181-219`).
    pub async fn start_device_auth(
        &self,
        cancel: Option<&CancelToken>,
    ) -> Result<DeviceAuthInfo, OAuthError> {
        let client = self.client(&self.endpoints.device_user_code_url).await?;
        // `:182-188`
        let send = client
            .post(&self.endpoints.device_user_code_url)
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "client_id": CLIENT_ID }))
            .send();
        let response = match with_cancel(send, cancel).await? {
            Ok(response) => response,
            Err(error) => return Err(map_fetch_error(&error, cancel)),
        };

        let status = response.status();
        if !status.is_success() {
            // `:190-195` — a 404 means the deployment has no device endpoint at all, which is a
            // different remedy from a transient failure, hence its own message.
            if status.as_u16() == 404 {
                return Err(OAuthError::Failed(DEVICE_NOT_ENABLED_MESSAGE.to_string()));
            }
            // `:196-199`, with `.catch(() => "")`.
            let body = with_cancel(response.text(), cancel)
                .await?
                .unwrap_or_default();
            return Err(OAuthError::Failed(format!(
                "OpenAI Codex device code request failed with status {}{}",
                status.as_u16(),
                body_suffix(&body)
            )));
        }

        // `:202-214`
        let text = match with_cancel(response.text(), cancel).await? {
            Ok(text) => text,
            Err(error) => return Err(map_fetch_error(&error, cancel)),
        };
        let json: serde_json::Value =
            serde_json::from_str(text.trim()).unwrap_or(serde_json::Value::Null);

        let device_auth_id = json
            .get("device_auth_id")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty());
        let user_code = json
            .get("user_code")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty());
        // `:206` — a string `interval` is coerced with `Number(...)`, anything else is used as-is.
        let interval_seconds = match json.get("interval") {
            Some(serde_json::Value::String(raw)) => js_number(raw),
            other => other.and_then(serde_json::Value::as_f64),
        };

        // `:207-214` — finite and non-negative; `Number.isFinite` also rejects `NaN`.
        match (device_auth_id, user_code, interval_seconds) {
            (Some(device_auth_id), Some(user_code), Some(interval_seconds))
                if interval_seconds.is_finite() && interval_seconds >= 0.0 =>
            {
                Ok(DeviceAuthInfo {
                    device_auth_id: device_auth_id.to_string(),
                    user_code: user_code.to_string(),
                    interval_seconds,
                })
            }
            _ => Err(OAuthError::Failed(format!(
                "Invalid OpenAI Codex device code response: {}",
                json_text(&json)
            ))),
        }
    }

    /// `pollOpenAICodexDeviceAuth` (`openai-codex.ts:221-274`). Note `waitBeforeFirstPoll` is
    /// **not** set, so the first poll is immediate.
    async fn poll_device_auth(
        &self,
        device: DeviceAuthInfo,
        cancel: Option<&CancelToken>,
    ) -> Result<DeviceTokenSuccess, OAuthError> {
        let options = DeviceCodePollOptions {
            // `:223-225`
            interval_seconds: Some(device.interval_seconds),
            expires_in_seconds: Some(DEVICE_CODE_TIMEOUT_SECONDS),
            wait_before_first_poll: false,
            cancel: cancel.cloned(),
        };
        let poller = DeviceTokenPoller {
            flow: self,
            device,
            cancel: cancel.cloned(),
        };
        poll_oauth_device_code_flow(&options, &poller).await
    }

    /// `loginOpenAICodexDeviceCode` (`openai-codex.ts:408-424`).
    async fn login_device_code(
        &self,
        interaction: &dyn AuthInteraction,
    ) -> Result<Credential, OAuthError> {
        let cancel = interaction.cancel().cloned();
        // `:409`
        let device = self.start_device_auth(cancel.as_ref()).await?;
        // `:410-416`
        interaction.notify(AuthEvent::DeviceCode {
            user_code: device.user_code.clone(),
            verification_uri: self.endpoints.device_verification_uri.clone(),
            interval_seconds: Some(device.interval_seconds),
            expires_in_seconds: Some(DEVICE_CODE_TIMEOUT_SECONDS),
        });
        // `:417`
        let code = self.poll_device_auth(device, cancel.as_ref()).await?;
        // `:418-423` — the device flow's code was minted against DEVICE_REDIRECT_URI, so the
        // exchange must present that, not REDIRECT_URI.
        let device_redirect_uri = self.endpoints.device_redirect_uri.clone();
        self.exchange_authorization_code_for_credentials(
            &code.authorization_code,
            &code.code_verifier,
            &device_redirect_uri,
            cancel.as_ref(),
        )
        .await
    }

    /// `startLocalOAuthServer` (`openai-codex.ts:303-375`).
    ///
    /// `Ok(None)` is upstream's `.on("error", ...)` branch (`:361-374`): the port could not be
    /// bound, so `waitForCode()` resolves `null` immediately and the login falls through to manual
    /// paste. Any other start failure (an already-aborted login) propagates.
    async fn start_local_oauth_server(
        &self,
        state: &str,
        interaction: &dyn AuthInteraction,
    ) -> Result<Option<CallbackServer<String>>, OAuthError> {
        // `:363` — bind `getCallbackHost()`, advertise `localhost`.
        let config = CallbackServerConfig::fixed(self.endpoints.callback_port, CALLBACK_PATH)
            .with_host(self.bind_host().await)
            .advertising(ADVERTISE_HOST)
            .with_interaction(interaction);
        match CallbackServer::start(
            config,
            CodexCallbackHandler {
                expected_state: state.to_string(),
            },
        )
        .await
        {
            Ok(server) => Ok(Some(server)),
            Err(OAuthError::Listen { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The body of `loginOpenAICodex` (`openai-codex.ts:426-486`) minus its `finally`, which
    /// [`Self::login_browser`] owns.
    async fn run_login_browser(
        &self,
        interaction: &dyn AuthInteraction,
        auth: &AuthorizationFlow,
        server: Option<&CallbackServer<String>>,
        manual_abort: &CancelToken,
    ) -> Result<Credential, OAuthError> {
        // `:434-438` — emitted before the manual prompt is armed.
        interaction.notify(AuthEvent::AuthUrl {
            url: auth.url.clone(),
            instructions: Some(AUTH_URL_INSTRUCTIONS.to_string()),
        });

        // `:441-456` — the prompt runs concurrently with the wait and calls `cancelWait` whichever
        // way it settles. Both futures are pinned rather than `select!`-owned because a dropped
        // `wait()` would take its result channel with it, and the paste branch still has to
        // consult the listener afterwards (`:458`).
        let mut manual = Box::pin(
            interaction.prompt(
                AuthPrompt::manual_code(MANUAL_PROMPT_MESSAGE)
                    .with_placeholder(self.endpoints.redirect_uri.clone())
                    .with_cancel(manual_abort.clone()),
            ),
        );

        enum Winner {
            Redirect(Result<Option<String>, OAuthError>),
            Manual(Result<String, OAuthError>),
        }

        let mut manual_input: Option<String> = None;
        let mut manual_error: Option<OAuthError> = None;

        let settled = match server {
            Some(server) => {
                let mut waiter = Box::pin(server.wait());
                let winner = tokio::select! {
                    settled = &mut waiter => Winner::Redirect(settled),
                    prompted = &mut manual => Winner::Manual(prompted),
                };
                match winner {
                    Winner::Redirect(settled) => settled?,
                    Winner::Manual(prompted) => {
                        match prompted {
                            // `:449-452`
                            Ok(input) => manual_input = Some(input),
                            // `:453-456`
                            Err(error) => manual_error = Some(error),
                        }
                        server.cancel_wait();
                        waiter.await?
                    }
                }
            }
            // `:371` — an unbound listener's `waitForCode()` resolves `null` at once.
            None => None,
        };

        // `:459` — a prompt rejection wins over whatever the listener said.
        if let Some(error) = manual_error {
            return Err(error);
        }

        let mut code: Option<String> = None;
        // `:460-466`
        if truthy(settled.as_deref()) {
            code = settled;
        } else if truthy(manual_input.as_deref()) {
            let parsed = parse_authorization_input(manual_input.as_deref().unwrap_or_default());
            // `:464` — a *present, non-empty* state that disagrees is fatal.
            if truthy(parsed.state.as_deref())
                && parsed.state.as_deref() != Some(auth.state.as_str())
            {
                return Err(OAuthError::Failed(STATE_MISMATCH_MESSAGE.to_string()));
            }
            code = parsed.code;
        }

        // `:468-476` — the second chance: `await manualPromise`. Reached when the listener
        // resolved without a code, which is `cancelWait` or an unbound listener.
        if !truthy(code.as_deref()) {
            if manual_input.is_none() && manual_error.is_none() {
                match manual.await {
                    Ok(input) => manual_input = Some(input),
                    // `:470-471` — `if (manualError) throw manualError`.
                    Err(error) => return Err(error),
                }
            }
            if truthy(manual_input.as_deref()) {
                let parsed = parse_authorization_input(manual_input.as_deref().unwrap_or_default());
                if truthy(parsed.state.as_deref())
                    && parsed.state.as_deref() != Some(auth.state.as_str())
                {
                    return Err(OAuthError::Failed(STATE_MISMATCH_MESSAGE.to_string()));
                }
                code = parsed.code;
            }
        }

        // `:478`
        if !truthy(code.as_deref()) {
            return Err(OAuthError::Failed(MISSING_CODE_MESSAGE.to_string()));
        }

        // `:479-485` — the browser flow always presents the *registered* redirect.
        let redirect_uri = self.endpoints.redirect_uri.clone();
        self.exchange_authorization_code_for_credentials(
            code.as_deref().unwrap_or_default(),
            &auth.verifier,
            &redirect_uri,
            interaction.cancel(),
        )
        .await
    }

    /// `loginOpenAICodex` (`openai-codex.ts:426-486`), including its `finally`.
    async fn login_browser(
        &self,
        interaction: &dyn AuthInteraction,
    ) -> Result<Credential, OAuthError> {
        // `:427`
        let auth = self.create_authorization_flow()?;
        // `:428`
        let server = self
            .start_local_oauth_server(&auth.state, interaction)
            .await?;
        // `:429`
        let manual_abort = CancelToken::new();

        let result = self
            .run_login_browser(interaction, &auth, server.as_ref(), &manual_abort)
            .await;

        // `:487-490` — `finally { manualAbort.abort(); server.close(); }`.
        manual_abort.cancel();
        if let Some(server) = &server {
            server.close();
        }
        result
    }
}

/// Race a future against the login-wide abort. Upstream hands `signal` straight to `fetch`
/// (`openai-codex.ts:115`); Rust has no ambient fetch, so the abort is a `select!`.
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

/// `fetchWithLoginCancellation`'s `catch` (`openai-codex.ts:114-120`): an aborted signal reports
/// `"Login cancelled"` whatever the transport said; everything else propagates.
fn map_fetch_error(error: &reqwest::Error, cancel: Option<&CancelToken>) -> OAuthError {
    if cancel.is_some_and(CancelToken::is_cancelled) {
        return OAuthError::Cancelled;
    }
    OAuthError::Failed(error.to_string())
}

#[async_trait::async_trait]
impl OAuthAuth for OpenAiCodexOAuthFlow {
    /// `openai-codex.ts:493`.
    fn name(&self) -> &str {
        OPENAI_CODEX_OAUTH_NAME
    }

    /// `isSubscription: true` (pi v0.84.1 `oauth/openai-codex.ts:517`) — a ChatGPT Plus/Pro plan.
    fn is_subscription(&self) -> bool {
        true
    }

    /// `openaiCodexOAuth.login` (`openai-codex.ts:495-532`): pick a method, then run it.
    ///
    /// The `select` answer is an option **id**, and anything other than the two known ids is
    /// upstream's `Unknown OpenAI Codex login method: {method}` (`:509`).
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        // `:496-506`
        let method = interaction
            .prompt(AuthPrompt::select(
                SELECT_LOGIN_METHOD_MESSAGE,
                vec![
                    AuthSelectOption {
                        id: BROWSER_LOGIN_METHOD.to_string(),
                        label: BROWSER_LOGIN_LABEL.to_string(),
                        description: None,
                    },
                    AuthSelectOption {
                        id: DEVICE_CODE_LOGIN_METHOD.to_string(),
                        label: DEVICE_CODE_LOGIN_LABEL.to_string(),
                        description: None,
                    },
                ],
            ))
            .await?;

        // `:508-514`
        if method == DEVICE_CODE_LOGIN_METHOD {
            return self.login_device_code(interaction).await;
        }
        if method != BROWSER_LOGIN_METHOD {
            return Err(OAuthError::Failed(format!(
                "Unknown OpenAI Codex login method: {method}"
            )));
        }
        self.login_browser(interaction).await
    }

    /// `openai-codex.ts:534` — `refresh: (credential) => refreshOpenAICodexToken(credential.refresh)`.
    /// Delegated; see the module note.
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        self.runtime.refresh(cred).await
    }

    /// `openai-codex.ts:536-538` — `{ apiKey: credential.access }`. Delegated; see the module note.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        self.runtime.to_auth(cred).await
    }
}

/// The provider id this flow's failures are attributed to when folded into
/// [`crate::error::AuthError`].
pub const PROVIDER_ID: &str = OPENAI_CODEX_PROVIDER_ID;

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
    use base64::Engine as _;
    use std::io::{Read as _, Write as _};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    // -- upstream-derived constants -----------------------------------------

    #[test]
    fn constants_are_upstream_verbatim() {
        // openai-codex.ts:26-39
        assert_eq!(CLIENT_ID, "app_EMoamEEZ73f0CkXaXp7hrann");
        assert_eq!(AUTH_BASE_URL, "https://auth.openai.com");
        assert_eq!(AUTHORIZE_URL, format!("{AUTH_BASE_URL}/oauth/authorize"));
        assert_eq!(TOKEN_URL, format!("{AUTH_BASE_URL}/oauth/token"));
        assert_eq!(REDIRECT_URI, "http://localhost:1455/auth/callback");
        assert_eq!(
            DEVICE_USER_CODE_URL,
            format!("{AUTH_BASE_URL}/api/accounts/deviceauth/usercode")
        );
        assert_eq!(
            DEVICE_TOKEN_URL,
            format!("{AUTH_BASE_URL}/api/accounts/deviceauth/token")
        );
        assert_eq!(
            DEVICE_VERIFICATION_URI,
            format!("{AUTH_BASE_URL}/codex/device")
        );
        assert_eq!(
            DEVICE_REDIRECT_URI,
            format!("{AUTH_BASE_URL}/deviceauth/callback")
        );
        assert_eq!(DEVICE_CODE_TIMEOUT_SECONDS, 900.0);
        assert_eq!(BROWSER_LOGIN_METHOD, "browser");
        assert_eq!(DEVICE_CODE_LOGIN_METHOD, "device_code");
        assert_eq!(SCOPE, "openid profile email offline_access");
        assert_eq!(JWT_CLAIM_PATH, "https://api.openai.com/auth");
        // The registered redirect decomposes into exactly the listener's bind target.
        assert_eq!(
            REDIRECT_URI,
            format!("http://{ADVERTISE_HOST}:{CALLBACK_PORT}{CALLBACK_PATH}")
        );
        // openai-codex.ts:493
        assert_eq!(
            OpenAiCodexOAuthFlow::new().name(),
            "OpenAI (ChatGPT Plus/Pro)"
        );
    }

    /// `openai-codex.ts:282-293` as `URLSearchParams.toString()` renders it: `scope`'s spaces
    /// become `+`, `redirect_uri`'s `://` and `/` become `%3A%2F%2F` and `%2F`.
    #[test]
    fn authorization_url_matches_url_search_params() {
        let flow = OpenAiCodexOAuthFlow::new();
        let auth = flow.create_authorization_flow().unwrap();
        let (base, query) = auth.url.split_once('?').unwrap();
        assert_eq!(base, AUTHORIZE_URL);
        assert_eq!(
            query,
            format!(
                concat!(
                    "response_type=code",
                    "&client_id=app_EMoamEEZ73f0CkXaXp7hrann",
                    "&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback",
                    "&scope=openid+profile+email+offline_access",
                    "&code_challenge={challenge}",
                    "&code_challenge_method=S256",
                    "&state={state}",
                    "&id_token_add_organizations=true",
                    "&codex_cli_simplified_flow=true",
                    "&originator=pi",
                ),
                challenge = super::super::pkce::pkce_challenge(&auth.verifier),
                state = auth.state,
            )
        );
        // `createState` (`:65-70`): 16 bytes as 32 lowercase hex characters, fresh per login.
        assert_eq!(auth.state.len(), 32);
        assert!(auth.state.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(auth.state.chars().all(|c| !c.is_ascii_uppercase()));
        assert_ne!(
            auth.state,
            flow.create_authorization_flow().unwrap().state,
            "state must not repeat between logins"
        );
    }

    // -- parseAuthorizationInput, openai-codex.ts:72-98 ---------------------

    /// The four branches `loginOpenAICodex` depends on. Asserted here even though the function is
    /// shared with `anthropic.rs`, so a change made there cannot silently change this flow.
    #[test]
    fn parse_authorization_input_covers_the_codex_branches() {
        // `:76-83` — a full redirect URL.
        let parsed =
            parse_authorization_input("  http://localhost:1455/auth/callback?code=abc&state=st  ");
        assert_eq!(parsed.code.as_deref(), Some("abc"));
        assert_eq!(parsed.state.as_deref(), Some("st"));
        // `:85-88` — `code#state`, with JS's truncating `split("#", 2)`.
        let parsed = parse_authorization_input("abc#st#extra");
        assert_eq!(parsed.code.as_deref(), Some("abc"));
        assert_eq!(parsed.state.as_deref(), Some("st"));
        // `:90-96` — a bare query string.
        let parsed = parse_authorization_input("code=abc&state=st");
        assert_eq!(parsed.code.as_deref(), Some("abc"));
        assert_eq!(parsed.state.as_deref(), Some("st"));
        // `:98` — a bare code carries no state, so `:464`'s mismatch check cannot fire.
        let parsed = parse_authorization_input("just-a-code");
        assert_eq!(parsed.code.as_deref(), Some("just-a-code"));
        assert_eq!(parsed.state, None);
        // `:74-75`
        assert_eq!(
            parse_authorization_input("   "),
            ParsedAuthorizationInput::default()
        );
    }

    // -- JWT fixtures -------------------------------------------------------

    /// Build a JWT whose payload carries `chatgpt_account_id`. STANDARD base64 (not base64url) is
    /// deliberate: `providers/openai_codex.rs`'s `atob` port rejects `-`/`_` exactly as the
    /// browser primitive upstream calls does.
    fn jwt_with_payload(payload: &serde_json::Value) -> String {
        let body = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_string(payload).unwrap());
        format!("header.{body}.signature")
    }

    fn jwt_for_account(account_id: &str) -> String {
        jwt_with_payload(&serde_json::json!({
            JWT_CLAIM_PATH: { "chatgpt_account_id": account_id }
        }))
    }

    // -- credentialsFromToken, openai-codex.ts:384-397 ----------------------

    #[test]
    fn credentials_from_token_stamps_the_account_id() {
        let access = jwt_for_account("acct_9f3");
        let cred = credentials_from_token(OAuthToken {
            access: access.clone(),
            refresh: "rt-1".into(),
            expires: 1_700_000_000_000,
        })
        .unwrap();
        match cred {
            Credential::Oauth {
                access: got,
                refresh,
                expires,
                ext,
            } => {
                assert_eq!(got, access);
                assert_eq!(refresh, "rt-1");
                assert_eq!(expires, 1_700_000_000_000);
                assert_eq!(
                    ext.get(EXT_ACCOUNT_ID).and_then(|v| v.as_str()),
                    Some("acct_9f3")
                );
            }
            other => panic!("expected an oauth credential, got {other:?}"),
        }
    }

    /// `:386-389` — a token with no usable claim fails with upstream's exact string, and NEVER
    /// yields a credential. Mirror: the same token shape *with* a claim succeeds (above).
    #[test]
    fn credentials_from_token_rejects_a_token_with_no_account_id() {
        for access in [
            "not-a-jwt".to_string(),
            jwt_with_payload(&serde_json::json!({})),
            jwt_with_payload(&serde_json::json!({ JWT_CLAIM_PATH: {} })),
            // `:381` — an empty account id is not a usable one.
            jwt_for_account(""),
        ] {
            let err = credentials_from_token(OAuthToken {
                access: access.clone(),
                refresh: "rt".into(),
                expires: 1,
            })
            .unwrap_err();
            assert_eq!(
                err.to_string(),
                "Failed to extract accountId from token",
                "access={access}"
            );
        }
    }

    // -- js_number, openai-codex.ts:206 -------------------------------------

    #[test]
    fn js_number_matches_the_number_coercion() {
        assert_eq!(js_number("5"), Some(5.0));
        assert_eq!(js_number(" 2.5 "), Some(2.5));
        assert_eq!(js_number("1e1"), Some(10.0));
        // `Number("")` and `Number("  ")` are both 0.
        assert_eq!(js_number(""), Some(0.0));
        assert_eq!(js_number("   "), Some(0.0));
        assert_eq!(js_number("nope"), None);
    }

    // -- a scriptable loopback HTTP endpoint --------------------------------

    /// Serves a scripted sequence of responses on loopback, one per connection, recording every
    /// request head and body. Never touches the network.
    struct FakeEndpoint {
        base: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl FakeEndpoint {
        fn start(responses: Vec<(u16, String)>) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&requests);
            let handle = std::thread::spawn(move || {
                for (status, body) in responses {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    let mut buf = [0u8; 16384];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let raw = String::from_utf8_lossy(&buf[..n]).to_string();
                    let (head, payload) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
                    sink.lock()
                        .unwrap()
                        .push((head.to_string(), payload.to_string()));
                    let reason = if (200..300).contains(&status) {
                        "OK"
                    } else {
                        "Error"
                    };
                    let response = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                }
            });
            FakeEndpoint {
                base: format!("http://127.0.0.1:{port}"),
                requests,
                handle: Some(handle),
            }
        }

        fn one(status: u16, body: &str) -> Self {
            Self::start(vec![(status, body.to_string())])
        }

        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base)
        }

        fn recorded(&mut self) -> Vec<(String, String)> {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
            self.requests.lock().unwrap().clone()
        }
    }

    fn flow_with_token_url(token_url: &str) -> OpenAiCodexOAuthFlow {
        OpenAiCodexOAuthFlow::with_endpoints(OpenAiCodexEndpoints {
            token_url: token_url.to_string(),
            callback_host: Some("127.0.0.1".to_string()),
            ..Default::default()
        })
    }

    // -- exchangeAuthorizationCode + readTokenResponse ----------------------

    #[tokio::test]
    async fn exchange_sends_the_upstream_form_body_and_stamps_expires() {
        let access = jwt_for_account("acct_1");
        let mut server = FakeEndpoint::one(
            200,
            &serde_json::json!({
                "access_token": access,
                "refresh_token": "rt-2",
                "expires_in": 3600,
            })
            .to_string(),
        );
        let flow = flow_with_token_url(&server.url("/oauth/token"));
        let before = now_ms();
        let token = flow
            .exchange_authorization_code("CODE", "VERIF", REDIRECT_URI, None)
            .await
            .unwrap();
        let after = now_ms();

        let recorded = server.recorded();
        let (head, body) = &recorded[0];
        assert!(head.starts_with("POST /oauth/token HTTP/1.1"), "{head}");
        assert!(
            head.to_lowercase()
                .contains("content-type: application/x-www-form-urlencoded"),
            "{head}"
        );
        // openai-codex.ts:152-157 — field order is upstream's insertion order.
        assert_eq!(
            body,
            "grant_type=authorization_code\
             &client_id=app_EMoamEEZ73f0CkXaXp7hrann\
             &code=CODE\
             &code_verifier=VERIF\
             &redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"
        );

        assert_eq!(token.access, access);
        assert_eq!(token.refresh, "rt-2");
        // `:144` — `Date.now() + expires_in * 1000`, with NO skew.
        assert!(
            token.expires >= before + 3_600_000 && token.expires <= after + 3_600_000,
            "expires={} before={before} after={after}",
            token.expires
        );
    }

    /// `:123-126` — the failure string names the operation and the status, and falls back to the
    /// reason phrase when the body is empty.
    #[tokio::test]
    async fn exchange_reports_http_failure_with_the_body_then_the_reason_phrase() {
        let mut server = FakeEndpoint::one(400, "invalid_grant");
        let err = flow_with_token_url(&server.url("/oauth/token"))
            .exchange_authorization_code("CODE", "VERIF", REDIRECT_URI, None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "OpenAI Codex token exchange failed (400): invalid_grant"
        );
        let _ = server.recorded();

        let server = FakeEndpoint::one(500, "");
        let err = flow_with_token_url(&server.url("/oauth/token"))
            .exchange_authorization_code("CODE", "VERIF", REDIRECT_URI, None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "OpenAI Codex token exchange failed (500): Internal Server Error"
        );
    }

    /// `:136-139` — every field is required, and `expires_in` must be a *number*. The message
    /// carries `JSON.stringify(json)`.
    #[tokio::test]
    async fn exchange_rejects_incomplete_token_responses() {
        // Every fixture's keys are already in sorted order, so the echoed payload is exactly what
        // `JSON.stringify` would emit; the reorder divergence is asserted separately below.
        for body in [
            r#"{"expires_in":1,"refresh_token":"r"}"#,
            r#"{"access_token":"a","expires_in":1}"#,
            r#"{"access_token":"a","refresh_token":"r"}"#,
            // `typeof json.expires_in !== "number"` (`:136`) — a numeric *string* is not a number.
            r#"{"access_token":"a","expires_in":"3600","refresh_token":"r"}"#,
            // JS truthiness (`:136`) — an empty access token is missing.
            r#"{"access_token":"","expires_in":1,"refresh_token":"r"}"#,
        ] {
            let server = FakeEndpoint::one(200, body);
            let err = flow_with_token_url(&server.url("/oauth/token"))
                .exchange_authorization_code("CODE", "VERIF", REDIRECT_URI, None)
                .await
                .unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("OpenAI Codex token exchange response missing fields: {body}"),
                "body={body}"
            );
        }
    }

    /// ACP-Q1 — **this test used to pin a divergence and now pins parity.**
    ///
    /// It was written because `serde_json::Map` was a `BTreeMap` here, so the echoed payload came
    /// out with sorted keys where `JSON.stringify` emits insertion order. `agent-client-protocol`
    /// (`cyrup-acp`'s wire dependency) declares `serde_json` with a non-optional `preserve_order`,
    /// and cargo feature unification is graph-wide, so the map is now an `IndexMap` and this
    /// user-visible error string is byte-identical to upstream's. The test is kept, and renamed,
    /// because the property is still worth pinning — it is just the opposite property now.
    #[tokio::test]
    async fn the_echoed_payload_matches_json_stringify_insertion_order() {
        let server = FakeEndpoint::one(200, r#"{"refresh_token":"r","expires_in":1}"#);
        let err = flow_with_token_url(&server.url("/oauth/token"))
            .exchange_authorization_code("CODE", "VERIF", REDIRECT_URI, None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            // Exactly what `JSON.stringify` emits for this body.
            r#"OpenAI Codex token exchange response missing fields: {"refresh_token":"r","expires_in":1}"#
        );
    }

    // -- startOpenAICodexDeviceAuth, openai-codex.ts:181-219 ----------------

    fn flow_with_device_urls(user_code_url: &str, token_url: &str) -> OpenAiCodexOAuthFlow {
        OpenAiCodexOAuthFlow::with_endpoints(OpenAiCodexEndpoints {
            device_user_code_url: user_code_url.to_string(),
            device_token_url: token_url.to_string(),
            callback_host: Some("127.0.0.1".to_string()),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn device_auth_start_parses_a_numeric_and_a_string_interval() {
        for (raw, expected) in [(r#"5"#, 5.0), (r#""5""#, 5.0), (r#""  0 ""#, 0.0)] {
            let body =
                format!(r#"{{"device_auth_id":"dev-1","user_code":"WXYZ-1234","interval":{raw}}}"#);
            let mut server = FakeEndpoint::one(200, &body);
            let flow = flow_with_device_urls(&server.url("/usercode"), DEVICE_TOKEN_URL);
            let device = flow.start_device_auth(None).await.unwrap();
            assert_eq!(
                device,
                DeviceAuthInfo {
                    device_auth_id: "dev-1".into(),
                    user_code: "WXYZ-1234".into(),
                    interval_seconds: expected,
                },
                "raw={raw}"
            );
            let recorded = server.recorded();
            let (head, payload) = &recorded[0];
            assert!(head.starts_with("POST /usercode HTTP/1.1"), "{head}");
            assert!(
                head.to_lowercase()
                    .contains("content-type: application/json"),
                "{head}"
            );
            // openai-codex.ts:185
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(payload).unwrap(),
                serde_json::json!({ "client_id": CLIENT_ID })
            );
        }
    }

    /// `:190-195` — a 404 gets its own remedy message, distinct from every other status.
    #[tokio::test]
    async fn device_auth_start_reports_404_as_not_enabled() {
        let server = FakeEndpoint::one(404, "nope");
        let err = flow_with_device_urls(&server.url("/usercode"), DEVICE_TOKEN_URL)
            .start_device_auth(None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "OpenAI Codex device code login is not enabled for this server. \
             Use browser login or verify the server URL."
        );

        // Mirror: any other status takes the generic branch, with the body appended.
        let server = FakeEndpoint::one(503, "upstream down");
        let err = flow_with_device_urls(&server.url("/usercode"), DEVICE_TOKEN_URL)
            .start_device_auth(None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "OpenAI Codex device code request failed with status 503: upstream down"
        );

        // `:197` — an empty body contributes no `: detail`.
        let server = FakeEndpoint::one(503, "");
        let err = flow_with_device_urls(&server.url("/usercode"), DEVICE_TOKEN_URL)
            .start_device_auth(None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "OpenAI Codex device code request failed with status 503"
        );
    }

    /// `:207-214` — every field required; a negative or non-finite interval is invalid.
    #[tokio::test]
    async fn device_auth_start_rejects_invalid_responses() {
        // Sorted-key fixtures, for the reason given on `echoed_payload_keys_are_sorted…`.
        for body in [
            r#"{"interval":5,"user_code":"U"}"#,
            r#"{"device_auth_id":"d","interval":5}"#,
            r#"{"device_auth_id":"d","user_code":"U"}"#,
            r#"{"device_auth_id":"d","interval":-1,"user_code":"U"}"#,
            r#"{"device_auth_id":"d","interval":"abc","user_code":"U"}"#,
        ] {
            let server = FakeEndpoint::one(200, body);
            let err = flow_with_device_urls(&server.url("/usercode"), DEVICE_TOKEN_URL)
                .start_device_auth(None)
                .await
                .unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("Invalid OpenAI Codex device code response: {body}"),
                "body={body}"
            );
        }
    }

    // -- pollOpenAICodexDeviceAuth, openai-codex.ts:221-274 -----------------

    async fn poll_once(status: u16, body: &str) -> DeviceCodePollResult<DeviceTokenSuccess> {
        let server = FakeEndpoint::one(status, body);
        let flow = flow_with_device_urls(DEVICE_USER_CODE_URL, &server.url("/token"));
        let poller = DeviceTokenPoller {
            flow: &flow,
            device: DeviceAuthInfo {
                device_auth_id: "dev-1".into(),
                user_code: "WXYZ".into(),
                interval_seconds: 1.0,
            },
            cancel: None,
        };
        poller.poll().await.unwrap()
    }

    #[tokio::test]
    async fn device_poll_maps_every_upstream_branch() {
        // `:238-247`
        assert_eq!(
            poll_once(
                200,
                r#"{"authorization_code":"ac-1","code_verifier":"cv-1"}"#
            )
            .await,
            DeviceCodePollResult::Complete(DeviceTokenSuccess {
                authorization_code: "ac-1".into(),
                code_verifier: "cv-1".into(),
            })
        );
        // `:241-245`
        assert_eq!(
            poll_once(200, r#"{"authorization_code":"ac-1"}"#).await,
            DeviceCodePollResult::Failed {
                message: r#"Invalid OpenAI Codex device auth token response: {"authorization_code":"ac-1"}"#
                    .to_string()
            }
        );
        // `:252-254` — the "not authorized yet" statuses.
        assert_eq!(poll_once(403, "").await, DeviceCodePollResult::Pending);
        assert_eq!(poll_once(404, "").await, DeviceCodePollResult::Pending);
        // `:265-267`
        assert_eq!(
            poll_once(400, r#"{"error":"deviceauth_authorization_pending"}"#).await,
            DeviceCodePollResult::Pending
        );
        // `:258-261` — a nested `{ code }` object is unwrapped.
        assert_eq!(
            poll_once(
                400,
                r#"{"error":{"code":"deviceauth_authorization_pending"}}"#
            )
            .await,
            DeviceCodePollResult::Pending
        );
        // `:268-270`
        assert_eq!(
            poll_once(429, r#"{"error":"slow_down"}"#).await,
            DeviceCodePollResult::SlowDown {
                interval_seconds: None
            }
        );
        // `:271-274` — anything else is terminal, with status and body.
        assert_eq!(
            poll_once(500, r#"{"error":"boom"}"#).await,
            DeviceCodePollResult::Failed {
                message: r#"OpenAI Codex device auth failed with status 500: {"error":"boom"}"#
                    .to_string()
            }
        );
        // A non-JSON body yields no error code, so it also takes the terminal branch.
        assert_eq!(
            poll_once(500, "<html>").await,
            DeviceCodePollResult::Failed {
                message: "OpenAI Codex device auth failed with status 500: <html>".to_string()
            }
        );
    }

    // -- loginOpenAICodexDeviceCode end-to-end ------------------------------

    /// The whole headless path through the public [`OAuthAuth::login`] entry point: pick the
    /// device method, receive a user code, poll once, exchange, and get a stamped credential.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn device_code_login_end_to_end() {
        let access = jwt_for_account("acct_dev");
        let mut user_code = FakeEndpoint::one(
            200,
            r#"{"device_auth_id":"dev-77","user_code":"ABCD-EFGH","interval":0}"#,
        );
        let mut device_token = FakeEndpoint::start(vec![
            // `:265-267` — one pending round trip, then success.
            (
                400,
                r#"{"error":"deviceauth_authorization_pending"}"#.to_string(),
            ),
            (
                200,
                r#"{"authorization_code":"ac-9","code_verifier":"cv-9"}"#.to_string(),
            ),
        ]);
        let mut token = FakeEndpoint::one(
            200,
            &serde_json::json!({
                "access_token": access,
                "refresh_token": "rt-dev",
                "expires_in": 60,
            })
            .to_string(),
        );

        let flow = OpenAiCodexOAuthFlow::with_endpoints(OpenAiCodexEndpoints {
            token_url: token.url("/oauth/token"),
            device_user_code_url: user_code.url("/usercode"),
            device_token_url: device_token.url("/deviceauth/token"),
            callback_host: Some("127.0.0.1".to_string()),
            ..Default::default()
        });

        let interaction = ScriptedInteraction::new(vec![Ok(DEVICE_CODE_LOGIN_METHOD.to_string())]);
        let credential = flow.login(&interaction).await.unwrap();

        // `:410-416` — the device-code event carries the *fixed* verification URI and timeout.
        assert_eq!(
            interaction.events(),
            vec![AuthEvent::DeviceCode {
                user_code: "ABCD-EFGH".to_string(),
                verification_uri: DEVICE_VERIFICATION_URI.to_string(),
                interval_seconds: Some(0.0),
                expires_in_seconds: Some(900.0),
            }]
        );
        // `:496-506` — the picker is a `select` over exactly two ids.
        let prompts = interaction.prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].message, "Select OpenAI Codex login method:");
        assert_eq!(
            prompts[0]
                .options
                .iter()
                .map(|o| (o.id.as_str(), o.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("browser", "Browser login (default)"),
                ("device_code", "Device code login (headless)"),
            ]
        );

        match &credential {
            Credential::Oauth {
                access: got,
                refresh,
                ext,
                ..
            } => {
                assert_eq!(got, &access);
                assert_eq!(refresh, "rt-dev");
                assert_eq!(
                    ext.get(EXT_ACCOUNT_ID).and_then(|v| v.as_str()),
                    Some("acct_dev")
                );
            }
            other => panic!("expected an oauth credential, got {other:?}"),
        }

        // `:418-423` — the device exchange presents DEVICE_REDIRECT_URI, not REDIRECT_URI.
        let exchanged = token.recorded();
        assert!(
            exchanged[0]
                .1
                .contains("&redirect_uri=https%3A%2F%2Fauth.openai.com%2Fdeviceauth%2Fcallback"),
            "{}",
            exchanged[0].1
        );
        assert!(
            exchanged[0].1.contains("&code=ac-9&code_verifier=cv-9"),
            "{}",
            exchanged[0].1
        );
        // Both device round trips carry the same identifiers (`:230-234`).
        for (_, payload) in device_token.recorded() {
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&payload).unwrap(),
                serde_json::json!({ "device_auth_id": "dev-77", "user_code": "ABCD-EFGH" })
            );
        }
        let _ = user_code.recorded();
    }

    // -- loginOpenAICodex (browser) end-to-end ------------------------------

    fn browser_get(url: &str) -> String {
        let rest = url.strip_prefix("http://").expect("http URL");
        let (authority, path) = rest.split_once('/').expect("URL has a path");
        let mut stream = std::net::TcpStream::connect(authority).expect("connect to listener");
        stream
            .write_all(
                format!("GET /{path} HTTP/1.1\r\nhost: {authority}\r\nconnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .expect("write request");
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        response
    }

    /// The redirect path: the listener receives `code` + `state`, the exchange presents the
    /// **registered** redirect URI, and the browser sees the success page.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn browser_login_completes_through_the_callback_listener() {
        let access = jwt_for_account("acct_web");
        let mut token = FakeEndpoint::one(
            200,
            &serde_json::json!({
                "access_token": access,
                "refresh_token": "rt-web",
                "expires_in": 120,
            })
            .to_string(),
        );
        let flow = OpenAiCodexOAuthFlow::with_endpoints(OpenAiCodexEndpoints {
            token_url: token.url("/oauth/token"),
            callback_host: Some("127.0.0.1".to_string()),
            callback_port: 0,
            ..Default::default()
        });

        let interaction = Arc::new(ScriptedInteraction::new(Vec::new()).blocking_when_empty());
        let auth = flow.create_authorization_flow().unwrap();
        let server = flow
            .start_local_oauth_server(&auth.state, interaction.as_ref())
            .await
            .unwrap()
            .expect("ephemeral bind succeeds");
        let port = server.port();
        let manual_abort = CancelToken::new();

        let login = {
            let interaction = Arc::clone(&interaction);
            let auth = auth.clone();
            let manual_abort = manual_abort.clone();
            async move {
                let result = flow
                    .run_login_browser(interaction.as_ref(), &auth, Some(&server), &manual_abort)
                    .await;
                manual_abort.cancel();
                server.close();
                result
            }
        };

        let state = auth.state.clone();
        let drive = tokio::task::spawn_blocking(move || {
            // Give the flow a moment to arm the manual prompt, then deliver the redirect.
            std::thread::sleep(Duration::from_millis(40));
            browser_get(&format!(
                "http://127.0.0.1:{port}/auth/callback?code=web-code&state={state}"
            ))
        });

        let (credential, response) = tokio::join!(login, drive);
        let response = response.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        // `:337`
        assert!(
            response.contains("<p>OpenAI authentication completed. You can close this window.</p>"),
            "{response}"
        );

        match credential.unwrap() {
            Credential::Oauth { refresh, ext, .. } => {
                assert_eq!(refresh, "rt-web");
                assert_eq!(
                    ext.get(EXT_ACCOUNT_ID).and_then(|v| v.as_str()),
                    Some("acct_web")
                );
            }
            other => panic!("expected an oauth credential, got {other:?}"),
        }

        // `:434-438`
        assert!(interaction.events().iter().any(|event| matches!(
            event,
            AuthEvent::AuthUrl { url, instructions }
                if url == &auth.url
                    && instructions.as_deref()
                        == Some("A browser window should open. Complete login to finish.")
        )));
        // `:441-448` — the manual prompt is armed with the registered redirect as placeholder.
        let prompts = interaction.prompts();
        assert_eq!(
            prompts[0].message,
            "Complete login in your browser, or paste the authorization code / redirect URL here:"
        );
        assert_eq!(prompts[0].placeholder.as_deref(), Some(REDIRECT_URI));

        // `:479-485` — the browser exchange presents REDIRECT_URI.
        let exchanged = token.recorded();
        assert_eq!(
            exchanged[0].1,
            "grant_type=authorization_code\
             &client_id=app_EMoamEEZ73f0CkXaXp7hrann\
             &code=web-code\
             &code_verifier="
                .to_string()
                + &auth.verifier
                + "&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"
        );
    }

    /// `:323-328` — a redirect whose `state` disagrees is answered with the 400 page and does NOT
    /// settle the login; the correct redirect that follows still completes it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn browser_callback_rejects_a_state_mismatch_without_settling() {
        let interaction = ScriptedInteraction::new(Vec::new()).blocking_when_empty();
        let flow = OpenAiCodexOAuthFlow::with_endpoints(OpenAiCodexEndpoints {
            callback_host: Some("127.0.0.1".to_string()),
            callback_port: 0,
            ..Default::default()
        });
        let server = flow
            .start_local_oauth_server("expected-state", &interaction)
            .await
            .unwrap()
            .expect("ephemeral bind succeeds");
        let port = server.port();

        let bad = tokio::task::spawn_blocking(move || {
            browser_get(&format!(
                "http://127.0.0.1:{port}/auth/callback?code=c&state=wrong"
            ))
        })
        .await
        .unwrap();
        assert!(bad.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{bad}");
        assert!(bad.contains("<p>State mismatch.</p>"), "{bad}");

        // `:329-334` — a matching state with no code is also answered without settling.
        let missing = tokio::task::spawn_blocking(move || {
            browser_get(&format!(
                "http://127.0.0.1:{port}/auth/callback?state=expected-state"
            ))
        })
        .await
        .unwrap();
        assert!(
            missing.contains("<p>Missing authorization code.</p>"),
            "{missing}"
        );

        // Mirror: the listener is still live, so a good redirect completes it.
        let good = tokio::task::spawn_blocking(move || {
            browser_get(&format!(
                "http://127.0.0.1:{port}/auth/callback?code=ok-code&state=expected-state"
            ))
        })
        .await
        .unwrap();
        assert!(good.starts_with("HTTP/1.1 200 OK\r\n"), "{good}");
        assert_eq!(server.wait().await.unwrap(), Some("ok-code".to_string()));
    }

    /// `:459-476` — the manual paste path. With no listener bound (`:371`), `waitForCode()`
    /// resolves `null` at once and the pasted redirect URL supplies the code.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn manual_paste_completes_when_the_listener_is_unbound() {
        let access = jwt_for_account("acct_paste");
        let mut token = FakeEndpoint::one(
            200,
            &serde_json::json!({
                "access_token": access,
                "refresh_token": "rt-paste",
                "expires_in": 30,
            })
            .to_string(),
        );
        let flow = flow_with_token_url(&token.url("/oauth/token"));
        let auth = flow.create_authorization_flow().unwrap();
        let pasted = format!("{REDIRECT_URI}?code=pasted-code&state={}", auth.state);
        let interaction = ScriptedInteraction::new(vec![Ok(pasted)]);
        let manual_abort = CancelToken::new();

        let credential = flow
            .run_login_browser(&interaction, &auth, None, &manual_abort)
            .await
            .unwrap();
        match credential {
            Credential::Oauth { refresh, .. } => assert_eq!(refresh, "rt-paste"),
            other => panic!("expected an oauth credential, got {other:?}"),
        }
        let exchanged = token.recorded();
        assert!(
            exchanged[0].1.contains("&code=pasted-code&"),
            "{}",
            exchanged[0].1
        );
    }

    /// `:464` / `:471` — a pasted redirect URL whose `state` disagrees is `"State mismatch"`,
    /// which is NOT Anthropic's `"OAuth state mismatch"`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn manual_paste_with_a_foreign_state_is_rejected() {
        // A LOOPBACK token endpoint, not `OpenAiCodexOAuthFlow::new()`. The mirror branch below
        // deliberately gets past the state check and reaches the exchange — with default endpoints
        // that is `https://auth.openai.com/oauth/token`, so this test POSTed a real
        // `grant_type=authorization_code` to OpenAI's production server on every
        // `cargo test -p cyrup-provider`. It came back 401 with a JSON body, which is a live
        // answer, and the assertion was environment-dependent besides: offline it passed on a
        // transport error, online on an auth error.
        let token = FakeEndpoint::start(vec![(400, "{\"error\":\"invalid_grant\"}".to_string())]);
        let flow = flow_with_token_url(&token.url("/oauth/token"));
        let auth = flow.create_authorization_flow().unwrap();
        let interaction = ScriptedInteraction::new(vec![Ok(format!(
            "{REDIRECT_URI}?code=c&state=someone-elses-state"
        ))]);
        let err = flow
            .run_login_browser(&interaction, &auth, None, &CancelToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "State mismatch");

        // Mirror: a paste carrying no state at all is accepted, because `:464` guards on a
        // *present* state. It then fails at the exchange, not at the state check.
        let interaction = ScriptedInteraction::new(vec![Ok("bare-code".to_string())]);
        let err = flow
            .run_login_browser(&interaction, &auth, None, &CancelToken::new())
            .await
            .unwrap_err();
        assert_ne!(err.to_string(), "State mismatch");
    }

    /// `:478` — nothing to exchange.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn empty_paste_reports_missing_authorization_code() {
        let flow = OpenAiCodexOAuthFlow::new();
        let auth = flow.create_authorization_flow().unwrap();
        let interaction = ScriptedInteraction::new(vec![Ok("   ".to_string())]);
        let err = flow
            .run_login_browser(&interaction, &auth, None, &CancelToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Missing authorization code");
    }

    /// `:509` — an id outside the two known ones.
    #[tokio::test]
    async fn unknown_login_method_is_reported_verbatim() {
        let interaction = ScriptedInteraction::new(vec![Ok("carrier-pigeon".to_string())]);
        let err = OpenAiCodexOAuthFlow::new()
            .login(&interaction)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Unknown OpenAI Codex login method: carrier-pigeon"
        );
    }

    /// An aborted login surfaces upstream's `"Login cancelled"` rather than hanging on the
    /// listener or the prompt.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn aborted_browser_login_reports_login_cancelled() {
        let cancel = CancelToken::new();
        let interaction = Arc::new(
            ScriptedInteraction::new(Vec::new())
                .blocking_when_empty()
                .with_cancel(cancel.clone()),
        );
        let flow = Arc::new(OpenAiCodexOAuthFlow::with_endpoints(OpenAiCodexEndpoints {
            callback_host: Some("127.0.0.1".to_string()),
            callback_port: 0,
            ..Default::default()
        }));
        let auth = flow.create_authorization_flow().unwrap();
        let server = flow
            .start_local_oauth_server(&auth.state, interaction.as_ref())
            .await
            .unwrap()
            .expect("ephemeral bind succeeds");

        let login = {
            let flow = Arc::clone(&flow);
            let interaction = Arc::clone(&interaction);
            tokio::spawn(async move {
                flow.run_login_browser(
                    interaction.as_ref(),
                    &auth,
                    Some(&server),
                    &CancelToken::new(),
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(40)).await;
        cancel.cancel();
        let err = match login.await.unwrap() {
            Ok(_) => panic!("an aborted login must not produce a credential"),
            Err(error) => error,
        };
        assert_eq!(err.to_string(), "Login cancelled");
    }

    /// `to_auth` is the delegated `{ apiKey: credential.access }` (`:536-538`).
    #[tokio::test]
    async fn to_auth_uses_the_access_token_as_the_bearer() {
        let flow = OpenAiCodexOAuthFlow::new();
        let cred = credentials_from_token(OAuthToken {
            access: jwt_for_account("acct_x"),
            refresh: "r".into(),
            expires: 1,
        })
        .unwrap();
        let auth = flow.to_auth(&cred).await.unwrap();
        let expected = match &cred {
            Credential::Oauth { access, .. } => access.clone(),
            other => panic!("expected an oauth credential, got {other:?}"),
        };
        assert_eq!(auth.api_key, Some(expected));
        assert_eq!(auth.base_url, None);
    }
}
