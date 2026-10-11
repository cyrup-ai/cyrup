//! Meta Model API OAuth flow — 1:1 port of pi `packages/ai/src/auth/oauth/meta.ts` @f1b2e77f5
//! (208 lines; landed v0.86.1, `b73412a37`, unchanged since). PROV-080.
//!
//! Upstream's header, verbatim (`meta.ts:1-13`):
//!
//! > RFC 8628 device authorization grant against <https://auth.meta.com> (JSON responses). Meta
//! > splits identity from API access: the resulting identity token is not accepted for inference,
//! > so it is exchanged for a Model API key via the Muse Code key-mint endpoint (minted keys live
//! > about a day). The identity token is stored as `refresh` and the minted key as `access`, so the
//! > standard OAuth scheduler re-mints the key when it expires with no bespoke renewal machinery.
//! > The identity token itself is not renewable (auth.meta.com answers grant_type=refresh_token
//! > with 404 and issues no refresh_token), so a 401/403 from mint means the session is dead and
//! > the user must sign in again.
//!
//! So [`OAuthAuth::refresh`] does not refresh anything at auth.meta.com: it re-runs the mint with
//! the stored identity token, and the ordinary refresh path in [`crate::auth::resolve`] drives it
//! when the day-lived key expires.
//!
//! ## Provenance
//!
//! | this module | `meta.ts` |
//! |---|---|
//! | [`CLIENT_ID`] / [`DEVICE_AUTHORIZATION_URL`] / [`DEVICE_TOKEN_URL`] / [`API_KEY_MINT_URL`] / [`API_KEY_LIFETIME_MS`] / [`REQUEST_TIMEOUT`] | `:19-26` |
//! | [`MetaOAuth::send`] | `requestSignal` and the three `fetch` calls, `:36-38` |
//! | [`read_json`] | `readJson`, `:40-47` |
//! | [`error_detail`] | `errorDetail`, `:49-55` |
//! | [`trusted_http_url`] | `trustedHttpUrl`, `:57-67` |
//! | [`positive_number`] | `positiveNumber`, `:69-71` |
//! | [`DeviceAuthorization`] / [`parse_device_authorization`] / [`MetaOAuth::start_device_authorization`] | `DeviceAuthorization`, `startDeviceAuthorization`, `:28-34`, `:73-100` |
//! | [`classify_token_reply`] / [`IdentityTokenPoller`] / [`MetaOAuth::poll_for_identity_token`] | `pollForIdentityToken`, `:102-143` |
//! | [`MetaOAuth::mint_api_key`] | `mintApiKey`, `:145-174` |
//! | [`MetaOAuth::run_login`] | `loginMeta`, `:176-194` |
//! | `impl OAuthAuth for MetaOAuth` | `metaOAuth`, `:196-208` |
//!
//! The poll loop (`pollOAuthDeviceCodeFlow`, `:103`) is [`super::device_code`]'s existing port; this
//! flow needed no change to it.
//!
//! ## Mechanism divergences (Rust forces these; behaviour is unchanged)
//!
//! * **No ambient `fetch`.** Requests go through `reqwest`, built by
//!   [`crate::stream::sse::build_client_for_target`] so they honour the same proxy policy as
//!   provider traffic, and the three endpoints are struct fields ([`MetaOAuth::with_endpoints`])
//!   because a test has no global `fetch` to stub. Production uses [`MetaOAuth::new`].
//! * **`AbortSignal` is [`CancelToken`].** `AbortSignal.any([AbortSignal.timeout(30s), signal])`
//!   (`:36-38`) is [`reqwest::RequestBuilder::timeout`] raced against [`CancelToken::cancelled`].
//!   A timed-out request fails with reqwest's text where pi's fails with the platform's
//!   `TimeoutError` text; neither is a stable contract.
//! * **No `signal` on [`OAuthAuth::refresh`].** pi's `refresh(credential, signal)` re-mints under
//!   the caller's signal; the trait has no parameter for one, so the trait method calls
//!   [`MetaOAuth::mint_api_key`] with `None` (the same note [`super::kimi_coding`] carries).
//! * **`JSON.stringify(json)`** in the invalid-device-response message is `serde_json::to_string`:
//!   the same key order (the workspace enables `preserve_order`), and the same text for every
//!   string, integer, boolean and null; a float with no fraction would print `1.0` where JS prints
//!   `1`.
//! * **`new URL(value).href`** is [`super::kimi_coding::trusted_http_url`] — pi's `meta.ts:57-67`
//!   is a copy of `kimi-coding.ts:57-67`, and cyrup keeps one Rust copy of the pair rather than
//!   three; see that function for the normalization it does and does not model.

use super::OAuthError;
use super::device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, DeviceCodePoller, poll_oauth_device_code_flow,
};
use super::interaction::{AuthEvent, AuthInteraction};
use super::query::encode_query;
use crate::auth::types::{AuthContext, Credential, EnvAuthContext, ModelAuth};
use crate::auth::{LoginOptions, OAuthAuth};
use crate::error::AuthError;
use cyrup_core::CancelToken;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants — meta.ts:19-26
// ---------------------------------------------------------------------------

/// "Muse Code CLI client id." (`meta.ts:19-20`). A public device-flow client; no secret.
pub const CLIENT_ID: &str = "1031625952748946";
/// `meta.ts:21`.
pub const AUTH_HOST: &str = "https://auth.meta.com";
/// `DEVICE_AUTHORIZATION_URL = `${AUTH_HOST}/oidc/device/authorization/`` (`meta.ts:22`) — note the
/// trailing slash.
pub const DEVICE_AUTHORIZATION_URL: &str = "https://auth.meta.com/oidc/device/authorization/";
/// `meta.ts:23`.
pub const DEVICE_TOKEN_URL: &str = "https://auth.meta.com/oidc/device/token/";
/// `meta.ts:24`.
pub const API_KEY_MINT_URL: &str = "https://api.meta.ai/muse-code/key";
/// `API_KEY_LIFETIME_MS = 24 * 60 * 60 * 1000` (`meta.ts:25`) — a minted key's `expires`, with no
/// skew subtracted.
pub const API_KEY_LIFETIME_MS: i64 = 24 * 60 * 60 * 1000;
/// `REQUEST_TIMEOUT_MS = 30 * 1000` (`meta.ts:26`), per request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// `grant_type` of the device-token poll (`meta.ts:116`).
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The provider these failures are attributed to.
pub const META_PROVIDER_ID: &str = "meta";

/// `meta.ts:186`.
pub const PROGRESS_MESSAGE: &str = "Enabling Meta Model API access...";

// ---------------------------------------------------------------------------
// Helpers — meta.ts:40-71
// ---------------------------------------------------------------------------

/// `readJson` (`meta.ts:40-47`): the body when it parses to a truthy `typeof === "object"` value
/// (an object or an array), otherwise `null`. The same function as `kimi-coding.ts:48-55`.
pub fn read_json(body: &str) -> Option<Value> {
    super::kimi_coding::read_json(body)
}

/// `errorDetail` (`meta.ts:49-55`): the first of `error_description`, `detail`, `message`, `error`
/// that is a string with non-blank content, trimmed and prefixed with `": "`; otherwise empty.
/// `json?.[key]` on an array reads nothing, so only an object can supply a detail.
pub fn error_detail(json: Option<&Value>) -> String {
    for key in ["error_description", "detail", "message", "error"] {
        if let Some(value) = json
            .and_then(Value::as_object)
            .and_then(|object| object.get(key))
            .and_then(Value::as_str)
        {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return format!(": {trimmed}");
            }
        }
    }
    String::new()
}

/// `trustedHttpUrl` (`meta.ts:57-67`): "The verification URI is opened in the user's browser; only
/// http(s) URLs are trusted." A non-string or empty value is `None`.
pub fn trusted_http_url(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .and_then(super::kimi_coding::trusted_http_url)
}

/// `positiveNumber` (`meta.ts:69-71`): a finite number above zero, else `undefined`.
pub fn positive_number(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && *number > 0.0)
}

/// `JSON.stringify(json)` for the invalid-response message; a rejected body is `null`, as pi's
/// `readJson` returns.
fn stringify(json: Option<&Value>) -> String {
    serde_json::to_string(json.unwrap_or(&Value::Null)).unwrap_or_else(|_| "null".to_string())
}

// ---------------------------------------------------------------------------
// DeviceAuthorization — meta.ts:28-34, :73-100
// ---------------------------------------------------------------------------

/// The parsed device-authorization reply (`DeviceAuthorization`, `meta.ts:28-34`).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    /// The URI shown to the user: `verification_uri_complete` when it is a trusted http(s) URL,
    /// else `verification_uri` (`meta.ts:89`).
    pub verification_uri: String,
    /// `positiveNumber(json?.interval)`; `None` lets the poller use RFC 8628's 5 s default.
    pub interval_seconds: Option<f64>,
    /// `positiveNumber(json?.expires_in)`; `None` means NO deadline (pi passes `undefined` to
    /// `pollOAuthDeviceCodeFlow`, which then polls until cancelled).
    pub expires_in_seconds: Option<f64>,
}

/// `startDeviceAuthorization`'s parse half (`meta.ts:87-99`), split out so every guard is
/// assertable without a socket. The status check that precedes it is in
/// [`MetaOAuth::start_device_authorization`].
pub fn parse_device_authorization(json: Option<&Value>) -> Result<DeviceAuthorization, OAuthError> {
    let get = |key: &str| json.and_then(Value::as_object).and_then(|o| o.get(key));
    let device_code = get("device_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let user_code = get("user_code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    // `trustedHttpUrl(json?.verification_uri_complete) ?? trustedHttpUrl(json?.verification_uri)`:
    // an untrusted complete URI falls back to the plain one rather than failing.
    let verification_uri = trusted_http_url(get("verification_uri_complete"))
        .or_else(|| trusted_http_url(get("verification_uri")));
    let (Some(device_code), Some(user_code), Some(verification_uri)) =
        (device_code, user_code, verification_uri)
    else {
        return Err(OAuthError::Failed(format!(
            "Invalid Meta device authorization response: {}",
            stringify(json)
        )));
    };
    Ok(DeviceAuthorization {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        verification_uri,
        interval_seconds: positive_number(get("interval")),
        expires_in_seconds: positive_number(get("expires_in")),
    })
}

/// One device-token reply mapped onto a poll result (`pollForIdentityToken`'s `poll`,
/// `meta.ts:120-140`), split out so every branch is assertable without a socket. The identity
/// token is the completed value. A success with no `access_token` and no `error` falls to the
/// default branch, as pi's `switch` does.
pub fn classify_token_reply(ok: bool, status: u16, json: Option<&Value>) -> DeviceCodePollResult<String> {
    let get = |key: &str| json.and_then(Value::as_object).and_then(|o| o.get(key));
    if ok
        && let Some(token) = get("access_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
    {
        return DeviceCodePollResult::Complete(token.to_string());
    }
    match get("error").and_then(Value::as_str) {
        Some("authorization_pending") => DeviceCodePollResult::Pending,
        Some("slow_down") => DeviceCodePollResult::SlowDown {
            interval_seconds: positive_number(get("interval")),
        },
        Some("access_denied") => DeviceCodePollResult::Failed {
            message: "Meta login was denied.".to_string(),
        },
        Some("expired_token") => DeviceCodePollResult::Failed {
            message: "Meta device authorization expired. Please restart login.".to_string(),
        },
        _ => DeviceCodePollResult::Failed {
            message: format!(
                "Meta device token request failed with status {status}{}",
                error_detail(json)
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------

/// One completed round trip: the status and the body as [`read_json`] reads it.
#[derive(Clone, Debug)]
struct HttpReply {
    status: u16,
    json: Option<Value>,
}

impl HttpReply {
    /// `response.ok`.
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The Meta (Muse subscription) OAuth strategy — upstream's `metaOAuth` (`meta.ts:196-208`).
pub struct MetaOAuth {
    device_authorization_url: String,
    device_token_url: String,
    api_key_mint_url: String,
    /// Ambient context for `HTTP(S)_PROXY` / `NO_PROXY` resolution.
    auth_ctx: Arc<dyn AuthContext>,
}

impl Default for MetaOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl MetaOAuth {
    /// The production strategy: upstream's three endpoints (`meta.ts:22-24`) and the real process
    /// environment.
    pub fn new() -> Self {
        Self::with_endpoints(
            DEVICE_AUTHORIZATION_URL,
            DEVICE_TOKEN_URL,
            API_KEY_MINT_URL,
            Arc::new(EnvAuthContext),
        )
    }

    /// Point the flow at other endpoints, under a given env/proxy source. Exists because Rust has
    /// no ambient `fetch` for a test to stub the way pi's `meta-oauth.test.ts` does.
    pub fn with_endpoints(
        device_authorization_url: impl Into<String>,
        device_token_url: impl Into<String>,
        api_key_mint_url: impl Into<String>,
        auth_ctx: Arc<dyn AuthContext>,
    ) -> Self {
        Self {
            device_authorization_url: device_authorization_url.into(),
            device_token_url: device_token_url.into(),
            api_key_mint_url: api_key_mint_url.into(),
            auth_ctx,
        }
    }

    /// One POST bounded by [`REQUEST_TIMEOUT`] and raced against `cancel` (`requestSignal`,
    /// `meta.ts:36-38`); the body is read with [`read_json`] before any status check, as every
    /// caller in `meta.ts` does.
    async fn send(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: String,
        cancel: Option<&CancelToken>,
    ) -> Result<HttpReply, OAuthError> {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(OAuthError::Cancelled);
        }
        let client =
            crate::stream::sse::build_client_for_target(url, self.auth_ctx.as_ref(), None, None)
                .await
                .map_err(|e| OAuthError::Failed(e.to_string()))?;
        let mut request = client.post(url).timeout(REQUEST_TIMEOUT).body(body);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let issue = async {
            let response = request
                .send()
                .await
                .map_err(|e| OAuthError::Failed(e.to_string()))?;
            let status = response.status().as_u16();
            // `readJson`'s `catch { return null }` covers a body that fails to read as well.
            let text = response.text().await.unwrap_or_default();
            Ok(HttpReply {
                status,
                json: read_json(&text),
            })
        };
        match cancel {
            Some(token) => tokio::select! {
                biased;
                () = token.cancelled() => Err(OAuthError::Cancelled),
                reply = issue => reply,
            },
            None => issue.await,
        }
    }

    /// A form POST with `Accept: application/json` (`meta.ts:75-80`, `:107-119`).
    async fn post_form(
        &self,
        url: &str,
        fields: &[(&str, &str)],
        cancel: Option<&CancelToken>,
    ) -> Result<HttpReply, OAuthError> {
        self.send(
            url,
            &[
                ("Content-Type", "application/x-www-form-urlencoded"),
                ("Accept", "application/json"),
            ],
            encode_query(fields.iter().copied()),
            cancel,
        )
        .await
    }

    /// `startDeviceAuthorization` (`meta.ts:73-100`).
    pub async fn start_device_authorization(
        &self,
        cancel: Option<&CancelToken>,
    ) -> Result<DeviceAuthorization, OAuthError> {
        let reply = self
            .post_form(
                &self.device_authorization_url,
                &[("client_id", CLIENT_ID)],
                cancel,
            )
            .await?;
        if !reply.ok() {
            return Err(OAuthError::Failed(format!(
                "Meta device authorization failed with status {}{}",
                reply.status,
                error_detail(reply.json.as_ref())
            )));
        }
        parse_device_authorization(reply.json.as_ref())
    }

    /// `pollForIdentityToken` (`meta.ts:102-143`): the identity token, which inference does not
    /// accept.
    pub async fn poll_for_identity_token(
        &self,
        device: &DeviceAuthorization,
        cancel: Option<&CancelToken>,
    ) -> Result<String, OAuthError> {
        let options = DeviceCodePollOptions {
            interval_seconds: device.interval_seconds,
            expires_in_seconds: device.expires_in_seconds,
            wait_before_first_poll: true,
            cancel: cancel.cloned(),
        };
        let poller = IdentityTokenPoller {
            flow: self,
            device_code: device.device_code.clone(),
            cancel: cancel.cloned(),
        };
        poll_oauth_device_code_flow(&options, &poller).await
    }

    /// `mintApiKey` (`meta.ts:145-174`): "Exchange an identity token for a Model API key. Keys are
    /// valid for about a day." The credential stores the identity token as `refresh` and the key as
    /// `access`, expiring [`API_KEY_LIFETIME_MS`] from now.
    pub async fn mint_api_key(
        &self,
        identity_token: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<Credential, OAuthError> {
        let bearer = format!("Bearer {identity_token}");
        let reply = self
            .send(
                &self.api_key_mint_url,
                &[
                    ("Accept", "application/json"),
                    ("Authorization", bearer.as_str()),
                    ("Content-Type", "application/json"),
                    ("x-api-version", "1.0.0"),
                ],
                "{}".to_string(),
                cancel,
            )
            .await?;
        let detail = error_detail(reply.json.as_ref());
        // "Identity token is not renewable (see file header); only a fresh device flow helps."
        if reply.status == 401 || reply.status == 403 {
            return Err(OAuthError::Failed(format!(
                "Meta session expired (status {}). Run `/login meta` to sign in again.{detail}",
                reply.status
            )));
        }
        if !reply.ok() {
            return Err(OAuthError::Failed(format!(
                "Meta API key mint failed with status {}{detail}",
                reply.status
            )));
        }
        let object = reply.json.as_ref().and_then(Value::as_object);
        let Some(api_key) = object
            .and_then(|o| o.get("api_key"))
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
        else {
            let setup = trusted_http_url(object.and_then(|o| o.get("action_url")))
                .map(|url| format!(" Complete setup at {url}"))
                .unwrap_or_default();
            return Err(OAuthError::Failed(format!(
                "Meta did not issue an API key.{setup}"
            )));
        };
        Ok(super::oauth_credential(
            api_key,
            identity_token,
            super::now_ms() + API_KEY_LIFETIME_MS,
        ))
    }

    /// `loginMeta` (`meta.ts:176-194`): device authorization, the device code shown, the identity
    /// token polled, a progress note, the key minted. "An in-flight fetch rejects with a
    /// DOMException on abort; the login UI matches on this message": once the login is cancelled,
    /// whatever failed is reported as `Login cancelled`.
    async fn run_login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        let cancel = interaction.cancel().cloned();
        let attempt = async {
            let device = self.start_device_authorization(cancel.as_ref()).await?;
            interaction.notify(AuthEvent::DeviceCode {
                user_code: device.user_code.clone(),
                verification_uri: device.verification_uri.clone(),
                interval_seconds: device.interval_seconds,
                expires_in_seconds: device.expires_in_seconds,
            });
            let identity = self
                .poll_for_identity_token(&device, cancel.as_ref())
                .await?;
            interaction.notify(AuthEvent::Progress {
                message: PROGRESS_MESSAGE.to_string(),
            });
            self.mint_api_key(&identity, cancel.as_ref()).await
        };
        attempt.await.map_err(|error| {
            if cancel.as_ref().is_some_and(CancelToken::is_cancelled) {
                OAuthError::Cancelled
            } else {
                error
            }
        })
    }
}

/// The `poll` callback of `pollForIdentityToken` (`meta.ts:107-141`).
struct IdentityTokenPoller<'a> {
    flow: &'a MetaOAuth,
    device_code: String,
    cancel: Option<CancelToken>,
}

#[async_trait::async_trait]
impl DeviceCodePoller for IdentityTokenPoller<'_> {
    type Value = String;

    async fn poll(&self) -> Result<DeviceCodePollResult<String>, OAuthError> {
        // `meta.ts:114-118`, in that field order. A transport failure rejects the whole flow, as
        // pi's unguarded `await fetch` does.
        let reply = self
            .flow
            .post_form(
                &self.flow.device_token_url,
                &[
                    ("grant_type", DEVICE_CODE_GRANT_TYPE),
                    ("device_code", self.device_code.as_str()),
                    ("client_id", CLIENT_ID),
                ],
                self.cancel.as_ref(),
            )
            .await?;
        Ok(classify_token_reply(
            reply.ok(),
            reply.status,
            reply.json.as_ref(),
        ))
    }
}

#[async_trait::async_trait]
impl OAuthAuth for MetaOAuth {
    /// `meta.ts:197`.
    fn name(&self) -> &str {
        "Meta (Muse subscription)"
    }

    /// `isSubscription: true` (`meta.ts:198`).
    fn is_subscription(&self) -> bool {
        true
    }

    /// `meta.ts:199`.
    fn login_label(&self) -> Option<&str> {
        Some("Sign in with Meta")
    }

    /// `login: loginMeta` (`meta.ts:201`).
    async fn login(
        &self,
        interaction: &dyn AuthInteraction,
        _options: &LoginOptions,
    ) -> Result<Credential, OAuthError> {
        self.run_login(interaction).await
    }

    /// `refresh: (credential, signal) => mintApiKey(credential.refresh, signal)` (`meta.ts:203`):
    /// a re-mint with the stored identity token, which auth.meta.com cannot itself renew.
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        let refresh = match cred {
            Credential::Oauth { refresh, .. } => refresh.clone(),
            Credential::ApiKey { .. } => {
                return Err(OAuthError::Failed(
                    "Meta OAuth refresh requires an oauth credential".to_string(),
                )
                .into_auth_error(META_PROVIDER_ID));
            }
        };
        self.mint_api_key(&refresh, None)
            .await
            .map_err(|e| e.into_auth_error(META_PROVIDER_ID))
    }

    /// `toAuth(credential) { return { apiKey: credential.access } }` (`meta.ts:205-207`): the
    /// minted key is an ordinary Model API key.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        match cred {
            Credential::Oauth { access, .. } => Ok(ModelAuth {
                api_key: Some(access.clone()),
                ..Default::default()
            }),
            Credential::ApiKey { .. } => Err(OAuthError::Failed(
                "Meta OAuth toAuth requires an oauth credential".to_string(),
            )
            .into_auth_error(META_PROVIDER_ID)),
        }
    }
}

#[cfg(test)]
mod tests;
