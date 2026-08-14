//! Radius gateway OAuth flow — port of pi v0.83.0 `packages/ai/src/auth/oauth/radius.ts`
//! (408 lines).
//!
//! Upstream's header: "Radius is a pi-messages gateway. OAuth client APIs live on the configured
//! gateway; only the interactive browser authorization endpoint is discovered. Model catalog
//! loading is owned by the Radius provider. NOTE: This module uses node:http for the OAuth
//! callback. It is only intended for CLI use, not browser environments." (`radius.ts:1-10`).
//!
//! Radius is the only flow whose endpoints are **not** constants: every OAuth call is made against
//! the gateway the user configured, and only the browser-authorization endpoint is discovered
//! (`GET /v1/oauth`). Like `openai-codex.ts` it offers two methods behind a `select` prompt, but
//! its device method *is* RFC 8628 — the same `/v1/oauth/token` endpoint with the device-code
//! grant type.
//!
//! ## Provenance
//!
//! | this module | `radius.ts` |
//! |---|---|
//! | [`CALLBACK_HOST`]/[`CALLBACK_PORT`]/[`CALLBACK_PATH`]/[`REDIRECT_URI`]/[`TOKEN_EXPIRY_SKEW_MS`] | `:27-31` |
//! | [`LOGIN_METHOD_BROWSER`]/[`LOGIN_METHOD_DEVICE_CODE`]/[`OAUTH_CLIENT_ID`]/[`OAUTH_SCOPE`]/[`OAUTH_DEVICE_CODE_GRANT_TYPE`] | `:32-36` |
//! | [`RadiusOAuthDiscovery`] | `RadiusOAuthDiscovery`, `:38-40` |
//! | [`DeviceAuthorizationResponse`] | `DeviceAuthorizationResponse`, `:42-48` |
//! | [`RadiusOAuth::load_oauth_discovery`] | `loadRadiusOAuthDiscovery`, `:50-65` |
//! | [`OAuthResponseError`] | `class OAuthResponseError`, `:67-82` |
//! | [`read_oauth_response_error`] | `readOAuthResponseError`, `:84-101` |
//! | [`RadiusOAuth::request_oauth_token`] | `requestOAuthToken`, `:103-140` |
//! | [`RadiusCallbackHandler`] + [`RadiusOAuth::start_oauth_callback_server`] | `startOAuthCallbackServer`, `:142-212` |
//! | [`RadiusOAuth::login_with_browser`] / [`RadiusOAuth::run_login_with_browser`] | `loginWithBrowser`, `:214-263` |
//! | [`RadiusOAuth::request_device_authorization`] | `requestDeviceAuthorization`, `:265-296` |
//! | [`DeviceTokenPoller`] + [`RadiusOAuth::login_with_device_code`] | `loginWithDeviceCode`, `:298-345` |
//! | [`RadiusOptions`] / [`create_radius_oauth`] / `impl OAuthAuth for RadiusOAuth` | `RadiusOAuthOptions`, `createRadiusOAuth`, `:347-408` |
//!
//! [`normalize_radius_gateway_url`] is `providers/radius-config.ts:52-55`, not `radius.ts` — but
//! `createRadiusOAuth` calls it on every construction (`:359`) and the Radius *provider* is not
//! ported yet, so it is defined here rather than left dangling. When
//! `providers/radius_config.rs` lands it should move there and this module should import it; see
//! this change's `not_done`.
//!
//! ## Mechanism divergences (Rust forces these; behaviour is unchanged unless noted)
//!
//! * **Callback server.** Upstream calls `node:http.createServer` (`:13-19`, `:174`); this crate's
//!   `tokio` has no `net` feature, so the listener is [`super::callback::CallbackServer`]. Three
//!   consequences, none reachable by a browser following a redirect:
//!   1. The shared server answers a foreign route with `"OAuth callback route not found."`, where
//!      `:176-179` says `"Callback route not found."`. Status (404) and page are otherwise
//!      identical; fixing it means changing the shared server, which is outside this module.
//!   2. `:186-191` calls `finish(null)` on an `?error=` redirect, so `waitForCode()` resolves
//!      `null` and `:227-232` then throws `"OAuth callback did not complete."`. The shared server
//!      has no "settle with none" outcome available to a handler, so the handler settles with that
//!      exact error instead — the same message reaches the caller, one step earlier.
//!   3. `:207-210` — a bind failure resolves `waitForCode()` with `null` rather than rejecting,
//!      which also lands on `"OAuth callback did not complete."`. Reproduced by mapping
//!      [`OAuthError::Listen`] to `None`; every other start failure still propagates.
//! * **No ambient `fetch`/`URL`.** Requests go through `reqwest`; `AbortSignal` is a
//!   [`CancelToken`] (arch-00 §3.2) raced against the request rather than handed to it, which is
//!   what `:113-119` and `:271-277` — "an aborted signal reports `Login cancelled`" — need.
//!   `new URL("/v1/oauth", gateway)` is [`gateway_url`], which resolves an absolute path against
//!   the gateway's **origin** exactly as the WHATWG parser does (any path on the configured
//!   gateway is therefore discarded, upstream's behaviour).
//! * **A token response missing its fields.** `:127-140` reads `data.access_token` unchecked, so a
//!   malformed response upstream yields a credential holding `undefined`/`NaN`. Rust's
//!   [`Credential::Oauth`] cannot represent that, so such a response is rejected with
//!   `"Radius OAuth token response is missing required fields"` — the phrasing `:293` already uses
//!   for the device-authorization response. Everything the fields *can* hold, including an empty
//!   string, is accepted exactly as upstream accepts it.
//! * **Endpoint override.** Upstream's tests stub the ambient `fetch`; Rust has none, so the
//!   callback bind target is [`RadiusOAuth::with_callback`]. The advertised [`REDIRECT_URI`] stays
//!   the upstream constant, because upstream sends the constant regardless of what bound.

use super::callback::{
    CallbackControl, CallbackHandler, CallbackOutcome, CallbackReply, CallbackRequest,
    CallbackServer, CallbackServerConfig,
};
use super::device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, DeviceCodePoller, poll_oauth_device_code_flow,
};
use super::interaction::{AuthEvent, AuthInteraction, AuthPrompt, AuthSelectOption};
use super::pkce::generate_pkce;
use super::query::encode_query;
use super::random::random_uuid_v4;
use super::{OAuthError, now_ms};
use crate::auth::OAuthAuth;
use crate::auth::types::{Credential, ModelAuth};
use crate::error::AuthError;
use cyrup_core::CancelToken;

pub use super::load::RadiusOptions;

// ---------------------------------------------------------------------------
// Constants — radius.ts:27-36
// ---------------------------------------------------------------------------

/// `CALLBACK_HOST` (`radius.ts:27`). Radius binds the loopback literal directly — it does **not**
/// consult `PI_OAUTH_CALLBACK_HOST` the way `openrouter.ts`/`anthropic.ts`/`openai-codex.ts` do.
pub const CALLBACK_HOST: &str = "127.0.0.1";
/// `CALLBACK_PORT` (`radius.ts:28`).
pub const CALLBACK_PORT: u16 = 1456;
/// `CALLBACK_PATH` (`radius.ts:29`).
pub const CALLBACK_PATH: &str = "/oauth/callback";
/// `REDIRECT_URI` (`radius.ts:30`) — a registered redirect, so the port is fixed.
pub const REDIRECT_URI: &str = "http://127.0.0.1:1456/oauth/callback";
/// `TOKEN_EXPIRY_SKEW_MS = 60_000` (`radius.ts:31`).
pub const TOKEN_EXPIRY_SKEW_MS: i64 = 60_000;
/// `LOGIN_METHOD_BROWSER` (`radius.ts:32`).
pub const LOGIN_METHOD_BROWSER: &str = "browser";
/// `LOGIN_METHOD_DEVICE_CODE` (`radius.ts:33`) — note the hyphen; `openai-codex.ts:37` spells its
/// equivalent id with an underscore.
pub const LOGIN_METHOD_DEVICE_CODE: &str = "device-code";
/// `OAUTH_CLIENT_ID` (`radius.ts:34`).
pub const OAUTH_CLIENT_ID: &str = "pi-gateway";
/// `OAUTH_SCOPE` (`radius.ts:35`). Space-separated; the urlencoded serializer turns the space
/// into `+`.
pub const OAUTH_SCOPE: &str = "gateway offline_access";
/// `OAUTH_DEVICE_CODE_GRANT_TYPE` (`radius.ts:36`) — RFC 8628 §3.4.
pub const OAUTH_DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// `radius.ts:229`.
const CALLBACK_INCOMPLETE_MESSAGE: &str = "OAuth callback did not complete.";
/// `radius.ts:246`.
const BROWSER_INSTRUCTIONS: &str = "Continue in your browser.";
/// `radius.ts:293`.
const DEVICE_MISSING_FIELDS_MESSAGE: &str =
    "Radius OAuth device authorization response is missing required fields";
/// See the module's divergence note; the phrasing mirrors [`DEVICE_MISSING_FIELDS_MESSAGE`].
const TOKEN_MISSING_FIELDS_MESSAGE: &str = "Radius OAuth token response is missing required fields";
/// `radius.ts:110`.
const TOKEN_REQUEST_FAILED_MESSAGE: &str = "Radius OAuth token request failed";
/// `radius.ts:281`.
const DEVICE_AUTHORIZATION_FAILED_MESSAGE: &str = "Radius OAuth device authorization failed";
/// `radius.ts:334`.
const DEVICE_EXPIRED_MESSAGE: &str = "Device authorization expired.";
/// `radius.ts:336`.
const DEVICE_DENIED_MESSAGE: &str = "Device authorization was denied.";

/// JS truthiness for an optional string: `undefined`/`null` and `""` are both falsy. Upstream
/// depends on it at `:186` (`if (error)`), `:198` (`if (!code)`) and in
/// `OAuthResponseError`'s detail template (`:75-79`).
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// normalizeRadiusGatewayUrl — providers/radius-config.ts:52-55
// ---------------------------------------------------------------------------

/// `normalizeRadiusGatewayUrl` (`providers/radius-config.ts:52-55`): add `https://` unless the
/// value already carries an `http`/`https` scheme (case-insensitively — `/^https?:\/\//iu`), then
/// strip every trailing `/`.
///
/// Note the regex only *tests*; a value that already has a scheme is passed through with its
/// original casing.
pub fn normalize_radius_gateway_url(value: &str) -> String {
    let has_scheme = {
        let lower = value.to_ascii_lowercase();
        lower.starts_with("http://") || lower.starts_with("https://")
    };
    let with_scheme = if has_scheme {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    with_scheme.trim_end_matches('/').to_string()
}

/// `new URL(path, gateway)` for an absolute `path` (`radius.ts:51`, `:105`, `:272`).
///
/// An absolute path resolves against the base's **origin**, so any path component the configured
/// gateway carries is discarded — the WHATWG behaviour upstream relies on.
fn gateway_url(gateway: &str, path: &str) -> String {
    match gateway.split_once("://") {
        Some((scheme, after)) => {
            let authority_end = after.find(['/', '?', '#']).unwrap_or(after.len());
            let authority = after.get(..authority_end).unwrap_or(after);
            format!("{scheme}://{authority}{path}")
        }
        // Unreachable after `normalize_radius_gateway_url`, which always supplies a scheme.
        None => format!("{gateway}{path}"),
    }
}

/// `url.search = params` (`radius.ts:224`): replace the query, keep everything else.
fn with_query(endpoint: &str, query: &str) -> String {
    let (before_fragment, fragment) = match endpoint.split_once('#') {
        Some((before, after)) => (before, Some(after)),
        None => (endpoint, None),
    };
    let base = before_fragment
        .split_once('?')
        .map_or(before_fragment, |(base, _)| base);
    match fragment {
        Some(fragment) => format!("{base}?{query}#{fragment}"),
        None => format!("{base}?{query}"),
    }
}

// ---------------------------------------------------------------------------
// Types — radius.ts:38-48
// ---------------------------------------------------------------------------

/// `RadiusOAuthDiscovery` (`radius.ts:38-40`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadiusOAuthDiscovery {
    pub authorization_endpoint: String,
}

/// `DeviceAuthorizationResponse` (`radius.ts:42-48`).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceAuthorizationResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: f64,
    pub interval: Option<f64>,
}

// ---------------------------------------------------------------------------
// OAuthResponseError — radius.ts:67-101
// ---------------------------------------------------------------------------

/// `class OAuthResponseError` (`radius.ts:67-82`).
///
/// Its `oauth_error` is what `loginWithDeviceCode` switches on (`:329-339`), which is why the poll
/// loop needs a *typed* failure rather than a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OAuthResponseError {
    pub status: u16,
    pub oauth_error: Option<String>,
    pub description: Option<String>,
    /// The prefix the throw site supplied (`:110`, `:281`).
    pub message: String,
}

impl std::fmt::Display for OAuthResponseError {
    /// `:73-79` — `${message}: ${detail}`, where `detail` prefers `error`, then
    /// `error: description`, then `description`, then the bare status. JS truthiness applies at
    /// both rungs, so an empty `error` or `error_description` falls through.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let oauth_error = self.oauth_error.as_deref().filter(|s| !s.is_empty());
        let description = self.description.as_deref().filter(|s| !s.is_empty());
        let detail = match (oauth_error, description) {
            (Some(error), Some(description)) => format!("{error}: {description}"),
            (Some(error), None) => error.to_string(),
            (None, Some(description)) => description.to_string(),
            (None, None) => self.status.to_string(),
        };
        write!(f, "{}: {detail}", self.message)
    }
}

impl std::error::Error for OAuthResponseError {}

/// `readOAuthResponseError` (`radius.ts:84-101`): a JSON body contributes `error` and
/// `error_description` when each is a string; a body that will not parse becomes the description
/// wholesale; an empty body contributes nothing.
pub fn read_oauth_response_error(status: u16, text: &str, message: &str) -> OAuthResponseError {
    let mut oauth_error: Option<String> = None;
    let mut description: Option<String> = None;

    // `:88` — JS truthiness: an empty body skips the whole block.
    if !text.is_empty() {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(data) => {
                oauth_error = data
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                description = data
                    .get("error_description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
            }
            // `:96-98`
            Err(_) => description = Some(text.to_string()),
        }
    }

    OAuthResponseError {
        status,
        oauth_error,
        description,
        message: message.to_string(),
    }
}

/// What a token request can fail with. The `Response` arm is upstream's `OAuthResponseError`,
/// which `loginWithDeviceCode`'s `instanceof` check (`:326-328`) depends on; everything else is a
/// transport/abort failure that upstream rethrows untouched.
#[derive(Debug)]
enum TokenRequestError {
    Response(OAuthResponseError),
    Other(OAuthError),
}

impl From<TokenRequestError> for OAuthError {
    fn from(error: TokenRequestError) -> OAuthError {
        match error {
            TokenRequestError::Response(response) => OAuthError::Failed(response.to_string()),
            TokenRequestError::Other(other) => other,
        }
    }
}

// ---------------------------------------------------------------------------
// The callback handler — radius.ts:174-201
// ---------------------------------------------------------------------------

/// The Radius half of the callback server (`radius.ts:174-201`).
pub struct RadiusCallbackHandler {
    expected_state: String,
}

#[async_trait::async_trait]
impl CallbackHandler for RadiusCallbackHandler {
    type Value = String;

    async fn handle(
        &self,
        request: CallbackRequest,
        _control: CallbackControl,
    ) -> CallbackOutcome<String> {
        // `:176-179` — the route check is the shared server's; see the module divergence note.
        // `:182-185`
        if request.param("state") != Some(self.expected_state.as_str()) {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(400, "OAuth state mismatch.", None),
            };
        }

        // `:187-192` — JS truthiness, so a bare `?error=` is not an error. This is the one branch
        // that ends the login without a code (`finish(null)`); see the module divergence note for
        // why it settles with the message upstream throws one step later.
        let error = request.param("error");
        if truthy(error) {
            let error = error.unwrap_or_default();
            // `:189` — `??`, so a present-but-empty `error_description` is kept.
            let description = request.param("error_description").unwrap_or(error);
            return CallbackOutcome::Failed {
                reply: CallbackReply::error(400, description, None),
                error: OAuthError::Failed(CALLBACK_INCOMPLETE_MESSAGE.to_string()),
            };
        }

        // `:194-199`
        let code = request.param("code");
        if !truthy(code) {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(400, "Missing authorization code.", None),
            };
        }

        // `:200-201`
        CallbackOutcome::Complete {
            reply: CallbackReply::success("Signed in to Radius. You may now close this page."),
            value: code.unwrap_or_default().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// The device-code poller — radius.ts:311-343
// ---------------------------------------------------------------------------

/// The `poll` closure of `loginWithDeviceCode` (`radius.ts:311-343`).
struct DeviceTokenPoller<'a> {
    flow: &'a RadiusOAuth,
    device_code: String,
    cancel: Option<CancelToken>,
}

#[async_trait::async_trait]
impl DeviceCodePoller for DeviceTokenPoller<'_> {
    type Value = Credential;

    async fn poll(&self) -> Result<DeviceCodePollResult<Credential>, OAuthError> {
        // `:313-322`
        let body = encode_query([
            ("grant_type", OAUTH_DEVICE_CODE_GRANT_TYPE),
            ("client_id", OAUTH_CLIENT_ID),
            ("device_code", self.device_code.as_str()),
        ]);
        match self
            .flow
            .request_oauth_token(&body, self.cancel.as_ref())
            .await
        {
            // `:323`
            Ok(credential) => Ok(DeviceCodePollResult::Complete(credential)),
            // `:325-328` — anything that is not an `OAuthResponseError` rethrows unchanged.
            Err(TokenRequestError::Other(error)) => Err(error),
            Err(TokenRequestError::Response(response)) => {
                // `:329-340`
                match response.oauth_error.as_deref() {
                    Some("authorization_pending") => Ok(DeviceCodePollResult::Pending),
                    // `:332-333` — Radius sends no new interval, so RFC 8628 §3.5's +5s applies.
                    Some("slow_down") => Ok(DeviceCodePollResult::SlowDown {
                        interval_seconds: None,
                    }),
                    Some("expired_token") => Ok(DeviceCodePollResult::Failed {
                        message: DEVICE_EXPIRED_MESSAGE.to_string(),
                    }),
                    Some("access_denied") => Ok(DeviceCodePollResult::Failed {
                        message: DEVICE_DENIED_MESSAGE.to_string(),
                    }),
                    // `:339` — `default: throw error`.
                    _ => Err(OAuthError::Failed(response.to_string())),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// createRadiusOAuth — radius.ts:347-408
// ---------------------------------------------------------------------------

/// A Radius gateway OAuth strategy — upstream's `createRadiusOAuth(options)`
/// (`radius.ts:353-408`).
#[derive(Clone, Debug)]
pub struct RadiusOAuth {
    name: String,
    /// `normalizeRadiusGatewayUrl(options.gateway)` (`:359`), applied once at construction.
    gateway: String,
    /// The listener's bind target. Production is [`CALLBACK_HOST`]/[`CALLBACK_PORT`]; a test may
    /// bind an ephemeral port. The *advertised* [`Self::redirect_uri`] is unaffected, because
    /// upstream always sends the [`REDIRECT_URI`] constant.
    callback_host: String,
    callback_port: u16,
    redirect_uri: String,
}

/// `createRadiusOAuth` (`radius.ts:353-408`).
pub fn create_radius_oauth(options: RadiusOptions) -> RadiusOAuth {
    RadiusOAuth::new(options)
}

impl RadiusOAuth {
    /// `createRadiusOAuth({ name, gateway })` (`radius.ts:353-360`).
    pub fn new(options: RadiusOptions) -> Self {
        Self {
            name: options.name,
            gateway: normalize_radius_gateway_url(&options.gateway),
            callback_host: CALLBACK_HOST.to_string(),
            callback_port: CALLBACK_PORT,
            redirect_uri: REDIRECT_URI.to_string(),
        }
    }

    /// Bind the callback listener somewhere else. Test-facing seam; production code uses
    /// [`RadiusOAuth::new`]. A port of `0` binds an ephemeral port.
    #[must_use]
    pub fn with_callback(mut self, host: impl Into<String>, port: u16) -> Self {
        self.callback_host = host.into();
        self.callback_port = port;
        self
    }

    /// The normalized gateway every OAuth call is made against.
    pub fn gateway(&self) -> &str {
        &self.gateway
    }

    /// The redirect the authorization server is told to come back to (`radius.ts:30`).
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// PROV-047: proxy-aware, per target. `build_client()` consulted neither the ported resolver
    /// nor the `httpProxy` setting, so every OAuth token exchange and silent refresh bypassed a
    /// configured proxy while provider streaming used it.
    async fn client(&self, target_url: &str) -> Result<reqwest::Client, OAuthError> {
        crate::stream::sse::build_client_for(target_url)
            .await
            .map_err(|e| OAuthError::Failed(e.to_string()))
    }

    /// `loadRadiusOAuthDiscovery` (`radius.ts:50-65`): `GET {gateway}/v1/oauth`.
    pub async fn load_oauth_discovery(&self) -> Result<RadiusOAuthDiscovery, OAuthError> {
        let url = gateway_url(&self.gateway, "/v1/oauth");
        let response = self
            .client(&url)
            .await?
            .get(&url)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|e| OAuthError::Failed(e.to_string()))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| OAuthError::Failed(e.to_string()))?;

        // `:56-60` — note the message names the gateway, not the URL, and interpolates the body
        // unconditionally.
        if !status.is_success() {
            return Err(OAuthError::Failed(format!(
                "Could not load Radius OAuth config from {}: {} {text}",
                self.gateway,
                status.as_u16()
            )));
        }

        // `:62-66`
        match serde_json::from_str::<serde_json::Value>(text.trim())
            .ok()
            .as_ref()
            .and_then(|json| json.get("authorizationEndpoint"))
            .and_then(serde_json::Value::as_str)
        {
            Some(endpoint) => Ok(RadiusOAuthDiscovery {
                authorization_endpoint: endpoint.to_string(),
            }),
            None => Err(OAuthError::Failed(format!(
                "Invalid Radius OAuth config from {}",
                self.gateway
            ))),
        }
    }

    /// `requestOAuthToken` (`radius.ts:103-140`): `POST {gateway}/v1/oauth/token`, form-encoded.
    async fn request_oauth_token(
        &self,
        body: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<Credential, TokenRequestError> {
        let url = gateway_url(&self.gateway, "/v1/oauth/token");
        let client = self
            .client(&url)
            .await
            .map_err(TokenRequestError::Other)?;

        // `:106-112`
        let send = client
            .post(&url)
            .header("accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body.to_string())
            .send();
        let response = match with_cancel(send, cancel)
            .await
            .map_err(TokenRequestError::Other)?
        {
            Ok(response) => response,
            // `:113-119` — an aborted signal reports `Login cancelled`, whatever the transport said.
            Err(error) => {
                return Err(TokenRequestError::Other(map_fetch_error(&error, cancel)));
            }
        };

        let status = response.status();
        let text = with_cancel(response.text(), cancel)
            .await
            .map_err(TokenRequestError::Other)?
            .unwrap_or_default();

        // `:121-123`
        if !status.is_success() {
            return Err(TokenRequestError::Response(read_oauth_response_error(
                status.as_u16(),
                &text,
                TOKEN_REQUEST_FAILED_MESSAGE,
            )));
        }

        // `:125-140`
        let data: serde_json::Value =
            serde_json::from_str(text.trim()).unwrap_or(serde_json::Value::Null);
        let access = data.get("access_token").and_then(serde_json::Value::as_str);
        let refresh = data.get("refresh_token").and_then(serde_json::Value::as_str);
        let expires_in = data.get("expires_in").and_then(serde_json::Value::as_f64);

        let (access, refresh, expires_in) = match (access, refresh, expires_in) {
            (Some(access), Some(refresh), Some(expires_in)) => (access, refresh, expires_in),
            // See the module's divergence note.
            _ => {
                return Err(TokenRequestError::Other(OAuthError::Failed(
                    TOKEN_MISSING_FIELDS_MESSAGE.to_string(),
                )));
            }
        };

        // `:139` — `scope` is copied through verbatim when the response carries the key at all,
        // which is what `{ ...credential, scope: data.scope }` does.
        let mut ext = serde_json::Map::new();
        if let Some(scope) = data.get("scope") {
            ext.insert("scope".to_string(), scope.clone());
        }

        // `:134-140` — note the skew is *subtracted*, so the stored deadline is a minute early.
        Ok(Credential::Oauth {
            refresh: refresh.to_string(),
            access: access.to_string(),
            expires: now_ms()
                .saturating_add((expires_in * 1000.0) as i64)
                .saturating_sub(TOKEN_EXPIRY_SKEW_MS),
            ext,
        })
    }

    /// `startOAuthCallbackServer` (`radius.ts:142-212`).
    ///
    /// `Ok(None)` is upstream's `.once("error", ...)` branch (`:207-210`); see the module
    /// divergence note.
    async fn start_oauth_callback_server(
        &self,
        expected_state: &str,
        interaction: &dyn AuthInteraction,
    ) -> Result<Option<CallbackServer<String>>, OAuthError> {
        let config = CallbackServerConfig::fixed(self.callback_port, CALLBACK_PATH)
            .with_host(self.callback_host.clone())
            .with_interaction(interaction);
        match CallbackServer::start(
            config,
            RadiusCallbackHandler {
                expected_state: expected_state.to_string(),
            },
        )
        .await
        {
            Ok(server) => Ok(Some(server)),
            Err(OAuthError::Listen { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The authorize URL `loginWithBrowser` opens (`radius.ts:222-233`). Parameter order is
    /// upstream's insertion order, which `URLSearchParams.toString()` preserves.
    pub fn authorization_url(
        &self,
        authorization_endpoint: &str,
        challenge: &str,
        state: &str,
    ) -> String {
        let params = encode_query([
            ("response_type", "code"),
            ("client_id", OAUTH_CLIENT_ID),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("scope", OAUTH_SCOPE),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            // `:231` — Radius-specific: tells the gateway to hand off by URL rather than inline.
            ("handoff", "url"),
            ("state", state),
        ]);
        with_query(authorization_endpoint, &params)
    }

    /// The body of `loginWithBrowser` (`radius.ts:235-262`) after the listener is up, minus the
    /// `finally`, which [`Self::login_with_browser`] owns.
    async fn run_login_with_browser(
        &self,
        interaction: &dyn AuthInteraction,
        authorize_url: &str,
        verifier: &str,
        server: Option<&CallbackServer<String>>,
    ) -> Result<Credential, OAuthError> {
        // `:236`
        interaction.notify(AuthEvent::Progress {
            message: format!("Listening for OAuth callback on {}", self.redirect_uri),
        });
        // `:237-241`
        interaction.notify(AuthEvent::AuthUrl {
            url: authorize_url.to_string(),
            instructions: Some(BROWSER_INSTRUCTIONS.to_string()),
        });

        // `:244`
        let code = match server {
            Some(server) => server.wait().await?,
            // `:209` — an unbound listener's `waitForCode()` resolves `null` at once.
            None => None,
        };

        // `:245-251`
        let Some(code) = code.filter(|code| !code.is_empty()) else {
            if interaction.cancel().is_some_and(CancelToken::is_cancelled) {
                return Err(OAuthError::Cancelled);
            }
            return Err(OAuthError::Failed(CALLBACK_INCOMPLETE_MESSAGE.to_string()));
        };

        // `:252-261`
        let body = encode_query([
            ("grant_type", "authorization_code"),
            ("client_id", OAUTH_CLIENT_ID),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("code", code.as_str()),
            ("code_verifier", verifier),
        ]);
        Ok(self
            .request_oauth_token(&body, interaction.cancel())
            .await?)
    }

    /// `loginWithBrowser` (`radius.ts:214-263`), including its `finally`.
    async fn login_with_browser(
        &self,
        authorization_endpoint: &str,
        interaction: &dyn AuthInteraction,
    ) -> Result<Credential, OAuthError> {
        // `:220-221`
        let pkce = generate_pkce()?;
        let state = random_uuid_v4()?;
        // `:222-233`
        let authorize_url = self.authorization_url(authorization_endpoint, &pkce.challenge, &state);
        // `:235`
        let server = self.start_oauth_callback_server(&state, interaction).await?;

        let result = self
            .run_login_with_browser(
                interaction,
                &authorize_url,
                &pkce.verifier,
                server.as_ref(),
            )
            .await;

        // `:262` — `finally { callbackServer.close(); }`.
        if let Some(server) = &server {
            server.close();
        }
        result
    }

    /// `requestDeviceAuthorization` (`radius.ts:265-296`): `POST {gateway}/v1/oauth/device`.
    pub async fn request_device_authorization(
        &self,
        cancel: Option<&CancelToken>,
    ) -> Result<DeviceAuthorizationResponse, OAuthError> {
        let url = gateway_url(&self.gateway, "/v1/oauth/device");
        let client = self.client(&url).await?;
        // `:268-276`
        let body = encode_query([("client_id", OAUTH_CLIENT_ID), ("scope", OAUTH_SCOPE)]);
        let send = client
            .post(&url)
            .header("accept", "application/json")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body)
            .send();
        let response = match with_cancel(send, cancel).await? {
            Ok(response) => response,
            // `:271-277`
            Err(error) => return Err(map_fetch_error(&error, cancel)),
        };

        let status = response.status();
        let text = with_cancel(response.text(), cancel)
            .await?
            .unwrap_or_default();

        // `:279-281`
        if !status.is_success() {
            return Err(OAuthError::Failed(
                read_oauth_response_error(
                    status.as_u16(),
                    &text,
                    DEVICE_AUTHORIZATION_FAILED_MESSAGE,
                )
                .to_string(),
            ));
        }

        // `:283-296` — JS truthiness on all four, so `""` and `0` are missing.
        let data: serde_json::Value =
            serde_json::from_str(text.trim()).unwrap_or(serde_json::Value::Null);
        let device_code = data
            .get("device_code")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty());
        let user_code = data
            .get("user_code")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty());
        let verification_uri = data
            .get("verification_uri")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty());
        let expires_in = data
            .get("expires_in")
            .and_then(serde_json::Value::as_f64)
            .filter(|value| *value != 0.0);

        match (device_code, user_code, verification_uri, expires_in) {
            (Some(device_code), Some(user_code), Some(verification_uri), Some(expires_in)) => {
                Ok(DeviceAuthorizationResponse {
                    device_code: device_code.to_string(),
                    user_code: user_code.to_string(),
                    verification_uri: verification_uri.to_string(),
                    expires_in,
                    interval: data.get("interval").and_then(serde_json::Value::as_f64),
                })
            }
            _ => Err(OAuthError::Failed(DEVICE_MISSING_FIELDS_MESSAGE.to_string())),
        }
    }

    /// `loginWithDeviceCode` (`radius.ts:298-345`).
    async fn login_with_device_code(
        &self,
        interaction: &dyn AuthInteraction,
    ) -> Result<Credential, OAuthError> {
        let cancel = interaction.cancel().cloned();
        // `:299`
        let device = self.request_device_authorization(cancel.as_ref()).await?;
        // `:300-306` — every hint here is the server's, unlike `openai-codex.ts:411-415`.
        interaction.notify(AuthEvent::DeviceCode {
            user_code: device.user_code.clone(),
            verification_uri: device.verification_uri.clone(),
            interval_seconds: device.interval,
            expires_in_seconds: Some(device.expires_in),
        });

        // `:308-311` — no `waitBeforeFirstPoll`, so the first poll is immediate.
        let options = DeviceCodePollOptions {
            interval_seconds: device.interval,
            expires_in_seconds: Some(device.expires_in),
            wait_before_first_poll: false,
            cancel: cancel.clone(),
        };
        let poller = DeviceTokenPoller {
            flow: self,
            device_code: device.device_code,
            cancel,
        };
        poll_oauth_device_code_flow(&options, &poller).await
    }
}

/// Race a future against the login-wide abort. Upstream hands `signal` straight to `fetch`
/// (`radius.ts:111`); Rust has no ambient fetch, so the abort is a `select!`.
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

/// `catch (error) { if (signal?.aborted) throw new Error("Login cancelled"); throw error; }`
/// (`radius.ts:113-119`, `:271-277`).
fn map_fetch_error(error: &reqwest::Error, cancel: Option<&CancelToken>) -> OAuthError {
    if cancel.is_some_and(CancelToken::is_cancelled) {
        return OAuthError::Cancelled;
    }
    OAuthError::Failed(error.to_string())
}

#[async_trait::async_trait]
impl OAuthAuth for RadiusOAuth {
    /// `radius.ts:362` — `name: options.name`, so the label is the gateway's, not a constant.
    fn name(&self) -> &str {
        &self.name
    }

    /// `radius.ts:364-386`: pick a method, then run it. Only the browser method loads discovery
    /// (`:383`) — the device method talks to fixed gateway paths.
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        // `:365-376`
        let method = interaction
            .prompt(AuthPrompt::select(
                format!("Sign in to {}:", self.name),
                vec![
                    AuthSelectOption {
                        id: LOGIN_METHOD_BROWSER.to_string(),
                        label: "Sign in with browser (recommended)".to_string(),
                        description: None,
                    },
                    AuthSelectOption {
                        id: LOGIN_METHOD_DEVICE_CODE.to_string(),
                        label: "Sign in with device code (when signing in from another device)"
                            .to_string(),
                        description: None,
                    },
                ],
            ))
            .await?;

        // `:378-385`
        if method == LOGIN_METHOD_DEVICE_CODE {
            return self.login_with_device_code(interaction).await;
        }
        if method == LOGIN_METHOD_BROWSER {
            let discovery = self.load_oauth_discovery().await?;
            return self
                .login_with_browser(&discovery.authorization_endpoint, interaction)
                .await;
        }
        // `:385`
        Err(OAuthError::Failed(format!(
            "Unknown {} sign-in method: {method}",
            self.name
        )))
    }

    /// `radius.ts:388-399` — a plain `refresh_token` grant against the same token endpoint.
    ///
    /// Upstream takes the caller's `signal`; the trait carries none, so the request is unraced.
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        let refresh = match cred {
            Credential::Oauth { refresh, .. } => refresh.as_str(),
            // Unrepresentable upstream, where `refresh(credential: OAuthCredential)` is typed.
            Credential::ApiKey { .. } => {
                return Err(OAuthError::Failed(
                    "Radius OAuth refresh requires an oauth credential".to_string(),
                )
                .into_auth_error("radius"));
            }
        };
        // `:391-397`
        let body = encode_query([
            ("grant_type", "refresh_token"),
            ("client_id", OAUTH_CLIENT_ID),
            ("refresh_token", refresh),
        ]);
        self.request_oauth_token(&body, None)
            .await
            .map_err(|e| OAuthError::from(e).into_auth_error("radius"))
    }

    /// `radius.ts:401-403` — `{ apiKey: credential.access }`.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        match cred {
            Credential::Oauth { access, .. } => Ok(ModelAuth {
                api_key: Some(access.clone()),
                ..Default::default()
            }),
            Credential::ApiKey { .. } => Err(OAuthError::Failed(
                "Radius OAuth toAuth requires an oauth credential".to_string(),
            )
            .into_auth_error("radius")),
        }
    }
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
    use crate::auth::oauth::interaction::ScriptedInteraction;
    use std::io::{Read as _, Write as _};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    // -- upstream-derived constants -----------------------------------------

    #[test]
    fn constants_are_upstream_verbatim() {
        // radius.ts:27-36
        assert_eq!(CALLBACK_HOST, "127.0.0.1");
        assert_eq!(CALLBACK_PORT, 1456);
        assert_eq!(CALLBACK_PATH, "/oauth/callback");
        assert_eq!(
            REDIRECT_URI,
            format!("http://{CALLBACK_HOST}:{CALLBACK_PORT}{CALLBACK_PATH}")
        );
        assert_eq!(TOKEN_EXPIRY_SKEW_MS, 60_000);
        assert_eq!(LOGIN_METHOD_BROWSER, "browser");
        assert_eq!(LOGIN_METHOD_DEVICE_CODE, "device-code");
        assert_eq!(OAUTH_CLIENT_ID, "pi-gateway");
        assert_eq!(OAUTH_SCOPE, "gateway offline_access");
        assert_eq!(
            OAUTH_DEVICE_CODE_GRANT_TYPE,
            "urn:ietf:params:oauth:grant-type:device_code"
        );
        // `:362` — the strategy name is the caller's, not a constant.
        assert_eq!(flow_for("https://gw.example").name(), "Acme Radius");
    }

    // -- normalizeRadiusGatewayUrl, providers/radius-config.ts:52-55 --------

    #[test]
    fn normalize_gateway_adds_https_and_strips_trailing_slashes() {
        assert_eq!(
            normalize_radius_gateway_url("radius.pi.dev"),
            "https://radius.pi.dev"
        );
        assert_eq!(
            normalize_radius_gateway_url("gw.example:8443/base/"),
            "https://gw.example:8443/base"
        );
        // Already-schemed values pass through untouched apart from the trailing slashes...
        assert_eq!(
            normalize_radius_gateway_url("http://localhost:9000///"),
            "http://localhost:9000"
        );
        // ...including their original casing: the regex only *tests*, it does not rewrite.
        assert_eq!(
            normalize_radius_gateway_url("HTTPS://GW.example/"),
            "HTTPS://GW.example"
        );
        // A non-http scheme is not matched, so it is treated as a bare host.
        assert_eq!(
            normalize_radius_gateway_url("ftp://gw.example"),
            "https://ftp://gw.example"
        );
        // Degenerate: `""` gains the scheme and then loses BOTH slashes to `/\/+$/`.
        assert_eq!(normalize_radius_gateway_url(""), "https:");
    }

    /// `new URL("/v1/oauth", gateway)` resolves against the **origin**, discarding any gateway
    /// path.
    #[test]
    fn gateway_url_resolves_absolute_paths_against_the_origin() {
        assert_eq!(
            gateway_url("https://gw.example", "/v1/oauth"),
            "https://gw.example/v1/oauth"
        );
        assert_eq!(
            gateway_url("https://gw.example:8443/base", "/v1/oauth/token"),
            "https://gw.example:8443/v1/oauth/token"
        );
        assert_eq!(
            gateway_url("http://127.0.0.1:9/a?b#c", "/v1/oauth/device"),
            "http://127.0.0.1:9/v1/oauth/device"
        );
    }

    /// `url.search = params` (`radius.ts:224`) replaces the query and keeps the fragment.
    #[test]
    fn with_query_replaces_the_search_component() {
        assert_eq!(with_query("https://a/b", "x=1"), "https://a/b?x=1");
        assert_eq!(with_query("https://a/b?old=9", "x=1"), "https://a/b?x=1");
        assert_eq!(
            with_query("https://a/b?old=9#frag", "x=1"),
            "https://a/b?x=1#frag"
        );
    }

    // -- OAuthResponseError, radius.ts:67-101 ------------------------------

    #[test]
    fn oauth_response_error_detail_follows_upstream_precedence() {
        let of = |body: &str| read_oauth_response_error(400, body, "Radius OAuth token request failed");
        // `:75-77` — error + description.
        assert_eq!(
            of(r#"{"error":"invalid_grant","error_description":"bad code"}"#).to_string(),
            "Radius OAuth token request failed: invalid_grant: bad code"
        );
        // `:77` — error alone.
        assert_eq!(
            of(r#"{"error":"invalid_grant"}"#).to_string(),
            "Radius OAuth token request failed: invalid_grant"
        );
        // `:78` — description alone.
        assert_eq!(
            of(r#"{"error_description":"bad code"}"#).to_string(),
            "Radius OAuth token request failed: bad code"
        );
        // `:78` — neither, so the bare status.
        assert_eq!(
            of(r#"{}"#).to_string(),
            "Radius OAuth token request failed: 400"
        );
        // `:88` — an empty body contributes nothing at all.
        assert_eq!(
            of("").to_string(),
            "Radius OAuth token request failed: 400"
        );
        // `:96-98` — an unparseable body becomes the description wholesale.
        assert_eq!(
            of("<html>gateway</html>").to_string(),
            "Radius OAuth token request failed: <html>gateway</html>"
        );
        // `:91-94` — non-string fields are ignored.
        assert_eq!(
            of(r#"{"error":7,"error_description":null}"#).to_string(),
            "Radius OAuth token request failed: 400"
        );
        // `oauth_error` is what the device poll switches on (`:329`).
        assert_eq!(
            of(r#"{"error":"authorization_pending"}"#).oauth_error.as_deref(),
            Some("authorization_pending")
        );
    }

    // -- a scriptable loopback gateway --------------------------------------

    /// Serves a scripted sequence of responses on loopback, one per connection, recording every
    /// request head and body. Never touches the network.
    struct FakeGateway {
        base: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl FakeGateway {
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
            FakeGateway {
                base: format!("http://127.0.0.1:{port}"),
                requests,
                handle: Some(handle),
            }
        }

        fn one(status: u16, body: &str) -> Self {
            Self::start(vec![(status, body.to_string())])
        }

        fn recorded(&mut self) -> Vec<(String, String)> {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
            self.requests.lock().unwrap().clone()
        }
    }

    fn flow_for(gateway: &str) -> RadiusOAuth {
        create_radius_oauth(RadiusOptions {
            name: "Acme Radius".to_string(),
            gateway: gateway.to_string(),
        })
    }

    // -- loadRadiusOAuthDiscovery, radius.ts:50-65 -------------------------

    #[tokio::test]
    async fn discovery_reads_the_authorization_endpoint() {
        let mut gateway = FakeGateway::one(
            200,
            r#"{"authorizationEndpoint":"https://auth.example/authorize"}"#,
        );
        let flow = flow_for(&gateway.base);
        let discovery = flow.load_oauth_discovery().await.unwrap();
        assert_eq!(
            discovery.authorization_endpoint,
            "https://auth.example/authorize"
        );
        let recorded = gateway.recorded();
        assert!(recorded[0].0.starts_with("GET /v1/oauth HTTP/1.1"), "{}", recorded[0].0);
        assert!(
            recorded[0].0.to_lowercase().contains("accept: application/json"),
            "{}",
            recorded[0].0
        );
    }

    /// `:56-60` and `:63-64` — both failure strings name the *gateway*.
    #[tokio::test]
    async fn discovery_failures_are_upstream_verbatim() {
        let gateway = FakeGateway::one(503, "down");
        let flow = flow_for(&gateway.base);
        assert_eq!(
            flow.load_oauth_discovery().await.unwrap_err().to_string(),
            format!("Could not load Radius OAuth config from {}: 503 down", flow.gateway())
        );

        for body in [r#"{}"#, r#"{"authorizationEndpoint":7}"#, "not json"] {
            let gateway = FakeGateway::one(200, body);
            let flow = flow_for(&gateway.base);
            assert_eq!(
                flow.load_oauth_discovery().await.unwrap_err().to_string(),
                format!("Invalid Radius OAuth config from {}", flow.gateway()),
                "body={body}"
            );
        }
    }

    // -- requestOAuthToken, radius.ts:103-140 ------------------------------

    #[tokio::test]
    async fn token_request_shape_expiry_skew_and_scope() {
        let mut gateway = FakeGateway::one(
            200,
            r#"{"access_token":"at-1","refresh_token":"rt-1","expires_in":3600,"scope":"gateway"}"#,
        );
        let flow = flow_for(&gateway.base);
        let before = now_ms();
        let cred = flow
            .request_oauth_token("grant_type=refresh_token&client_id=pi-gateway", None)
            .await
            .unwrap();
        let after = now_ms();

        let recorded = gateway.recorded();
        assert!(
            recorded[0].0.starts_with("POST /v1/oauth/token HTTP/1.1"),
            "{}",
            recorded[0].0
        );
        let lower = recorded[0].0.to_lowercase();
        assert!(lower.contains("accept: application/json"), "{lower}");
        assert!(
            lower.contains("content-type: application/x-www-form-urlencoded"),
            "{lower}"
        );

        match cred {
            Credential::Oauth {
                access,
                refresh,
                expires,
                ext,
            } => {
                assert_eq!(access, "at-1");
                assert_eq!(refresh, "rt-1");
                // `:137` — `Date.now() + expires_in * 1000 - 60_000`. The skew must be
                // *subtracted*: an unskewed deadline would land at or after `after + 3_600_000`.
                assert!(
                    expires >= before + 3_600_000 - TOKEN_EXPIRY_SKEW_MS
                        && expires <= after + 3_600_000 - TOKEN_EXPIRY_SKEW_MS,
                    "expires={expires} before={before} after={after}"
                );
                assert!(
                    expires < before + 3_600_000,
                    "the 60s skew was not subtracted: expires={expires}"
                );
                // `:139`
                assert_eq!(ext.get("scope").and_then(|v| v.as_str()), Some("gateway"));
            }
            other => panic!("expected an oauth credential, got {other:?}"),
        }
    }

    /// `:139` — `scope` rides through only when the response carries the key. Mirror of the case
    /// above, so the assertion there is not vacuous.
    #[tokio::test]
    async fn token_response_without_scope_stores_no_scope() {
        let gateway = FakeGateway::one(
            200,
            r#"{"access_token":"at","refresh_token":"rt","expires_in":10}"#,
        );
        let cred = flow_for(&gateway.base)
            .request_oauth_token("grant_type=refresh_token", None)
            .await
            .unwrap();
        match cred {
            Credential::Oauth { ext, .. } => assert!(ext.is_empty(), "{ext:?}"),
            other => panic!("expected an oauth credential, got {other:?}"),
        }
    }

    /// `:121-123` — a non-2xx becomes an `OAuthResponseError`, which is what the device poll
    /// switches on.
    #[tokio::test]
    async fn token_request_non_2xx_yields_a_typed_response_error() {
        let gateway = FakeGateway::one(400, r#"{"error":"authorization_pending"}"#);
        let err = flow_for(&gateway.base)
            .request_oauth_token("grant_type=x", None)
            .await
            .unwrap_err();
        match err {
            TokenRequestError::Response(response) => {
                assert_eq!(response.status, 400);
                assert_eq!(response.oauth_error.as_deref(), Some("authorization_pending"));
                assert_eq!(
                    response.to_string(),
                    "Radius OAuth token request failed: authorization_pending"
                );
            }
            other => panic!("expected a typed response error, got {other:?}"),
        }
    }

    // -- requestDeviceAuthorization, radius.ts:265-296 ---------------------

    #[tokio::test]
    async fn device_authorization_shape_and_validation() {
        let mut gateway = FakeGateway::one(
            200,
            r#"{"device_code":"dc-1","user_code":"WXYZ","verification_uri":"https://gw/dev","expires_in":600,"interval":3}"#,
        );
        let flow = flow_for(&gateway.base);
        let device = flow.request_device_authorization(None).await.unwrap();
        assert_eq!(
            device,
            DeviceAuthorizationResponse {
                device_code: "dc-1".into(),
                user_code: "WXYZ".into(),
                verification_uri: "https://gw/dev".into(),
                expires_in: 600.0,
                interval: Some(3.0),
            }
        );
        let recorded = gateway.recorded();
        assert!(
            recorded[0].0.starts_with("POST /v1/oauth/device HTTP/1.1"),
            "{}",
            recorded[0].0
        );
        // `:274` — the field order upstream serializes.
        assert_eq!(recorded[0].1, "client_id=pi-gateway&scope=gateway+offline_access");

        // `:288-294` — JS truthiness, so an empty string or a zero `expires_in` is missing.
        for body in [
            r#"{"user_code":"U","verification_uri":"v","expires_in":1}"#,
            r#"{"device_code":"d","verification_uri":"v","expires_in":1}"#,
            r#"{"device_code":"d","user_code":"U","expires_in":1}"#,
            r#"{"device_code":"d","user_code":"U","verification_uri":"v"}"#,
            r#"{"device_code":"d","user_code":"U","verification_uri":"v","expires_in":0}"#,
            r#"{"device_code":"","user_code":"U","verification_uri":"v","expires_in":1}"#,
        ] {
            let gateway = FakeGateway::one(200, body);
            let err = flow_for(&gateway.base)
                .request_device_authorization(None)
                .await
                .unwrap_err();
            assert_eq!(
                err.to_string(),
                "Radius OAuth device authorization response is missing required fields",
                "body={body}"
            );
        }

        // `:279-281` — a non-2xx carries the device-specific message prefix.
        let gateway = FakeGateway::one(403, r#"{"error":"unauthorized_client"}"#);
        let err = flow_for(&gateway.base)
            .request_device_authorization(None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Radius OAuth device authorization failed: unauthorized_client"
        );
    }

    // -- the device poll mapping, radius.ts:325-340 ------------------------

    async fn poll_once(status: u16, body: &str) -> Result<DeviceCodePollResult<Credential>, OAuthError> {
        let gateway = FakeGateway::one(status, body);
        let flow = flow_for(&gateway.base);
        let poller = DeviceTokenPoller {
            flow: &flow,
            device_code: "dc-1".to_string(),
            cancel: None,
        };
        poller.poll().await
    }

    /// `Credential` is not `PartialEq`, so the poll results are compared by description rather
    /// than by value. The description keeps the failure *message* — the part upstream specifies.
    fn describe(result: &DeviceCodePollResult<Credential>) -> String {
        match result {
            DeviceCodePollResult::Pending => "pending".to_string(),
            DeviceCodePollResult::SlowDown { interval_seconds } => {
                format!("slow_down:{interval_seconds:?}")
            }
            DeviceCodePollResult::Failed { message } => format!("failed:{message}"),
            DeviceCodePollResult::Complete(_) => "complete".to_string(),
        }
    }

    #[tokio::test]
    async fn device_poll_maps_every_upstream_oauth_error() {
        // `:330-331`
        assert_eq!(
            describe(
                &poll_once(400, r#"{"error":"authorization_pending"}"#)
                    .await
                    .unwrap()
            ),
            "pending"
        );
        // `:332-333`
        assert_eq!(
            describe(&poll_once(400, r#"{"error":"slow_down"}"#).await.unwrap()),
            "slow_down:None"
        );
        // `:334-335`
        assert_eq!(
            describe(&poll_once(400, r#"{"error":"expired_token"}"#).await.unwrap()),
            "failed:Device authorization expired."
        );
        // `:336-337`
        assert_eq!(
            describe(&poll_once(400, r#"{"error":"access_denied"}"#).await.unwrap()),
            "failed:Device authorization was denied."
        );
        // `:338-339` — an unrecognised code rethrows rather than being folded into a poll result.
        let err = poll_once(400, r#"{"error":"invalid_client"}"#)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Radius OAuth token request failed: invalid_client"
        );
        // `:323` — and a 2xx completes.
        assert!(matches!(
            poll_once(
                200,
                r#"{"access_token":"at","refresh_token":"rt","expires_in":5}"#
            )
            .await
            .unwrap(),
            DeviceCodePollResult::Complete(_)
        ));
    }

    // -- loginWithDeviceCode end-to-end ------------------------------------

    /// The whole headless path through the public [`OAuthAuth::login`] entry point.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn device_code_login_end_to_end() {
        let mut gateway = FakeGateway::start(vec![
            (
                200,
                r#"{"device_code":"dc-9","user_code":"AAAA-BBBB","verification_uri":"https://gw.example/device","expires_in":900,"interval":0}"#
                    .to_string(),
            ),
            (400, r#"{"error":"authorization_pending"}"#.to_string()),
            (
                200,
                r#"{"access_token":"at-9","refresh_token":"rt-9","expires_in":600}"#.to_string(),
            ),
        ]);
        let flow = flow_for(&gateway.base);
        let interaction = ScriptedInteraction::new(vec![Ok(LOGIN_METHOD_DEVICE_CODE.to_string())]);
        let credential = flow.login(&interaction).await.unwrap();

        // `:300-306` — every hint comes from the server's response.
        assert_eq!(
            interaction.events(),
            vec![AuthEvent::DeviceCode {
                user_code: "AAAA-BBBB".to_string(),
                verification_uri: "https://gw.example/device".to_string(),
                interval_seconds: Some(0.0),
                expires_in_seconds: Some(900.0),
            }]
        );
        // `:365-376` — the picker names the gateway and offers exactly two ids.
        let prompts = interaction.prompts();
        assert_eq!(prompts[0].message, "Sign in to Acme Radius:");
        assert_eq!(
            prompts[0]
                .options
                .iter()
                .map(|o| (o.id.as_str(), o.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("browser", "Sign in with browser (recommended)"),
                (
                    "device-code",
                    "Sign in with device code (when signing in from another device)"
                ),
            ]
        );

        match credential {
            Credential::Oauth { access, refresh, .. } => {
                assert_eq!(access, "at-9");
                assert_eq!(refresh, "rt-9");
            }
            other => panic!("expected an oauth credential, got {other:?}"),
        }

        let recorded = gateway.recorded();
        assert!(recorded[1].0.starts_with("POST /v1/oauth/token"), "{}", recorded[1].0);
        // `:313-322` — the device grant, in upstream's field order.
        assert_eq!(
            recorded[1].1,
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code\
             &client_id=pi-gateway\
             &device_code=dc-9"
        );
    }

    // -- loginWithBrowser end-to-end ---------------------------------------

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

    /// `:222-233` as `URLSearchParams.toString()` renders it, including Radius's `handoff=url`.
    #[test]
    fn authorization_url_matches_url_search_params() {
        let flow = flow_for("https://gw.example");
        assert_eq!(
            flow.authorization_url("https://auth.example/authorize?stale=1", "CHAL", "STATE"),
            concat!(
                "https://auth.example/authorize",
                "?response_type=code",
                "&client_id=pi-gateway",
                "&redirect_uri=http%3A%2F%2F127.0.0.1%3A1456%2Foauth%2Fcallback",
                "&scope=gateway+offline_access",
                "&code_challenge=CHAL",
                "&code_challenge_method=S256",
                "&handoff=url",
                "&state=STATE",
            )
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn browser_login_completes_through_the_callback_listener() {
        let mut gateway = FakeGateway::one(
            200,
            r#"{"access_token":"at-web","refresh_token":"rt-web","expires_in":300}"#,
        );
        let flow = flow_for(&gateway.base).with_callback("127.0.0.1", 0);
        let interaction = Arc::new(ScriptedInteraction::new(Vec::new()));
        let server = flow
            .start_oauth_callback_server("state-abc", interaction.as_ref())
            .await
            .unwrap()
            .expect("ephemeral bind succeeds");
        let port = server.port();

        let authorize_url =
            flow.authorization_url("https://auth.example/authorize", "CHAL", "state-abc");
        let login = {
            let interaction = Arc::clone(&interaction);
            let flow = flow.clone();
            let authorize_url = authorize_url.clone();
            async move {
                flow.run_login_with_browser(
                    interaction.as_ref(),
                    &authorize_url,
                    "VERIF",
                    Some(&server),
                )
                .await
            }
        };
        let drive = tokio::task::spawn_blocking(move || {
            std::thread::sleep(Duration::from_millis(40));
            browser_get(&format!(
                "http://127.0.0.1:{port}/oauth/callback?code=web-code&state=state-abc"
            ))
        });

        let (credential, response) = tokio::join!(login, drive);
        let response = response.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        // `:201`
        assert!(
            response.contains("<p>Signed in to Radius. You may now close this page.</p>"),
            "{response}"
        );

        match credential.unwrap() {
            Credential::Oauth { access, .. } => assert_eq!(access, "at-web"),
            other => panic!("expected an oauth credential, got {other:?}"),
        }

        // `:236-241` — the progress line names the *registered* redirect, not the bound port.
        assert_eq!(
            interaction.events(),
            vec![
                AuthEvent::Progress {
                    message: format!("Listening for OAuth callback on {REDIRECT_URI}"),
                },
                AuthEvent::AuthUrl {
                    url: authorize_url,
                    instructions: Some("Continue in your browser.".to_string()),
                },
            ]
        );

        // `:252-261` — the code exchange, in upstream's field order.
        let recorded = gateway.recorded();
        assert_eq!(
            recorded[0].1,
            "grant_type=authorization_code\
             &client_id=pi-gateway\
             &redirect_uri=http%3A%2F%2F127.0.0.1%3A1456%2Foauth%2Fcallback\
             &code=web-code\
             &code_verifier=VERIF"
        );
    }

    /// `:182-199` — a state mismatch and a missing code are both answered without ending the
    /// login, so a good redirect afterwards still completes it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn callback_rejects_bad_redirects_without_settling() {
        let flow = flow_for("https://gw.example").with_callback("127.0.0.1", 0);
        let interaction = ScriptedInteraction::new(Vec::new());
        let server = flow
            .start_oauth_callback_server("state-abc", &interaction)
            .await
            .unwrap()
            .expect("ephemeral bind succeeds");
        let port = server.port();

        let bad = tokio::task::spawn_blocking(move || {
            browser_get(&format!(
                "http://127.0.0.1:{port}/oauth/callback?code=c&state=wrong"
            ))
        })
        .await
        .unwrap();
        assert!(bad.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{bad}");
        assert!(bad.contains("<p>OAuth state mismatch.</p>"), "{bad}");

        let missing = tokio::task::spawn_blocking(move || {
            browser_get(&format!(
                "http://127.0.0.1:{port}/oauth/callback?state=state-abc"
            ))
        })
        .await
        .unwrap();
        assert!(missing.contains("<p>Missing authorization code.</p>"), "{missing}");

        // Mirror: the listener is still live.
        let good = tokio::task::spawn_blocking(move || {
            browser_get(&format!(
                "http://127.0.0.1:{port}/oauth/callback?code=ok&state=state-abc"
            ))
        })
        .await
        .unwrap();
        assert!(good.starts_with("HTTP/1.1 200 OK\r\n"), "{good}");
        assert_eq!(server.wait().await.unwrap(), Some("ok".to_string()));
    }

    /// `:186-191` + `:227-232` — an `?error=` redirect renders the description and ends the login
    /// with `"OAuth callback did not complete."`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn callback_error_redirect_ends_the_login() {
        let flow = flow_for("https://gw.example").with_callback("127.0.0.1", 0);
        let interaction = Arc::new(ScriptedInteraction::new(Vec::new()));
        let server = flow
            .start_oauth_callback_server("state-abc", interaction.as_ref())
            .await
            .unwrap()
            .expect("ephemeral bind succeeds");
        let port = server.port();

        let login = {
            let interaction = Arc::clone(&interaction);
            let flow = flow.clone();
            async move {
                flow.run_login_with_browser(interaction.as_ref(), "https://a/b", "VERIF", Some(&server))
                    .await
            }
        };
        let drive = tokio::task::spawn_blocking(move || {
            std::thread::sleep(Duration::from_millis(40));
            browser_get(&format!(
                "http://127.0.0.1:{port}/oauth/callback?state=state-abc&error=access_denied&error_description=User+said+no"
            ))
        });
        let (result, response) = tokio::join!(login, drive);
        let response = response.unwrap();
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{response}");
        // `:189` — the description wins over the bare code.
        assert!(response.contains("<p>User said no</p>"), "{response}");
        assert_eq!(
            result.unwrap_err().to_string(),
            "OAuth callback did not complete."
        );
    }

    /// `:207-210` + `:227-232` — an unbound listener resolves `null`, which is the same failure.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn unbound_listener_reports_that_the_callback_did_not_complete() {
        let flow = flow_for("https://gw.example");
        let interaction = ScriptedInteraction::new(Vec::new());
        let err = flow
            .run_login_with_browser(&interaction, "https://a/b", "VERIF", None)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "OAuth callback did not complete.");

        // `:246-248` — but an aborted login says so instead.
        let cancel = CancelToken::new();
        cancel.cancel();
        let interaction = ScriptedInteraction::new(Vec::new()).with_cancel(cancel);
        let err = flow
            .run_login_with_browser(&interaction, "https://a/b", "VERIF", None)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Login cancelled");
    }

    // -- refresh / toAuth, radius.ts:388-403 -------------------------------

    #[tokio::test]
    async fn refresh_posts_the_refresh_grant_and_returns_a_new_credential() {
        let mut gateway = FakeGateway::one(
            200,
            r#"{"access_token":"at-2","refresh_token":"rt-2","expires_in":120}"#,
        );
        let flow = flow_for(&gateway.base);
        let cred = Credential::Oauth {
            refresh: "rt-1".into(),
            access: "at-1".into(),
            expires: 0,
            ext: serde_json::Map::new(),
        };
        let refreshed = flow.refresh(&cred).await.unwrap();
        match refreshed {
            Credential::Oauth { access, refresh, .. } => {
                assert_eq!(access, "at-2");
                assert_eq!(refresh, "rt-2");
            }
            other => panic!("expected an oauth credential, got {other:?}"),
        }
        // `:391-397`
        let recorded = gateway.recorded();
        assert_eq!(
            recorded[0].1,
            "grant_type=refresh_token&client_id=pi-gateway&refresh_token=rt-1"
        );
    }

    #[tokio::test]
    async fn refresh_failure_folds_into_the_oauth_taxonomy() {
        let gateway = FakeGateway::one(400, r#"{"error":"invalid_grant"}"#);
        let flow = flow_for(&gateway.base);
        let err = flow
            .refresh(&Credential::Oauth {
                refresh: "rt-1".into(),
                access: "at-1".into(),
                expires: 0,
                ext: serde_json::Map::new(),
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), "oauth");
        // `AuthError::OAuth`'s own Display names only the provider (`error.rs:20`); upstream's
        // message is the wrapped cause, which is what a caller prints when it walks the chain.
        let cause = std::error::Error::source(&err).expect("the oauth failure carries its cause");
        assert_eq!(
            cause.to_string(),
            "Radius OAuth token request failed: invalid_grant"
        );
    }

    #[tokio::test]
    async fn to_auth_uses_the_access_token_as_the_bearer() {
        let flow = flow_for("https://gw.example");
        let auth = flow
            .to_auth(&Credential::Oauth {
                refresh: "r".into(),
                access: "at-x".into(),
                expires: 0,
                ext: serde_json::Map::new(),
            })
            .await
            .unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("at-x"));
        assert_eq!(auth.base_url, None);

        // Mirror: an api-key credential is not a Radius OAuth credential.
        assert!(flow.to_auth(&Credential::api_key("sk")).await.is_err());
    }

    /// `:385` — an id outside the two known ones.
    #[tokio::test]
    async fn unknown_login_method_names_the_gateway() {
        let interaction = ScriptedInteraction::new(vec![Ok("smoke-signal".to_string())]);
        let err = flow_for("https://gw.example")
            .login(&interaction)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Unknown Acme Radius sign-in method: smoke-signal"
        );
    }
}
