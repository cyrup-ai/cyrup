//! xAI OAuth device-code flow — 1:1 port of pi v0.83.0 `packages/ai/src/auth/oauth/xai.ts`
//! (238 lines). Upstream's header: "xAI OAuth device-code flow." (`xai.ts:1-3`).
//!
//! This is the SuperGrok / X Premium subscription login. It is RFC 8628, not a redirect flow: no
//! callback listener, no PKCE. The user is shown a code and a verification URI and this module
//! polls the token endpoint until they finish in a browser.
//!
//! ## Provenance
//!
//! | this module | `xai.ts` |
//! |---|---|
//! | [`CLIENT_ID`] / [`SCOPE`] / [`DEVICE_CODE_URL`] / [`TOKEN_URL`] / [`REFRESH_SKEW_MS`] / [`DEFAULT_TOKEN_LIFETIME_SECONDS`] | `:8-14` |
//! | [`HttpResponse`] | `OAuthHttpResponse`, `:18-22` |
//! | [`XaiDeviceCode`] | `XaiDeviceCode`, `:24-31` |
//! | [`required_string`] | `requiredString`, `:33-39` |
//! | [`positive_number`] | `positiveNumber`, `:41-47` |
//! | [`validate_verification_uri`] | `validateVerificationUri`, `:49-62` |
//! | [`post_form`] | `postForm`, `:64-98` |
//! | [`request_failure`] | `requestFailure`, `:100-106` |
//! | [`parse_device_code`] | `parseDeviceCode`, `:108-126` |
//! | [`credentials_from_token_response`] | `credentialsFromTokenResponse`, `:128-143` |
//! | [`XaiOAuth::request_device_code`] | `requestDeviceCode`, `:145-159` |
//! | [`XaiTokenPoller`] | the `poll` closure of `pollForTokens`, `:161-199` |
//! | [`XaiOAuth::run_login`] | `loginXai`, `:201-211` |
//! | [`XaiOAuth::refresh_token`] | `refreshXaiToken`, `:213-227` |
//! | `impl OAuthAuth for XaiOAuth` | `xaiOAuth`, `:229-238` |
//!
//! The poll loop itself (`pollOAuthDeviceCodeFlow`, `:162`) is
//! [`super::device_code::poll_oauth_device_code_flow`], already ported from `device-code.ts`;
//! `URLSearchParams` serialization is [`super::query::encode_query`].
//!
//! ## Mechanism divergences (Rust forces these; behaviour is unchanged)
//!
//! * **No ambient `fetch`.** [`post_form`] is `reqwest`, and `AbortSignal` is a [`CancelToken`]
//!   (arch-00 §3.2), raced against the request rather than handed to it. `:77-79` and `:88-90` —
//!   "an aborted signal reports `Login cancelled`, whatever the transport said" — are preserved.
//! * **No `new URL`.** [`validate_verification_uri`] hand-rolls the part of the WHATWG parser this
//!   check depends on; see its own doc for exactly what is normalized and what is passed through.
//!   The security property upstream's comment names (`:49-50`: "force it to be an https URL so a
//!   malicious response cannot make `open` launch something else") is exact.
//! * **JSON typing.** `typeof value !== "string"` / `!== "number"` become
//!   [`serde_json::Value::as_str`] / [`serde_json::Value::as_f64`], which reject the same values.
//!   `body.refresh_token === undefined` (`:132`) is `!body.contains_key("refresh_token")`: a JSON
//!   `null` is **present**, so it takes the `requiredString` branch and fails, exactly as JS does.
//! * **Endpoint override.** Upstream's tests stub the ambient `fetch`; Rust has none, so the two
//!   endpoints are struct fields ([`XaiOAuth::with_endpoints`]) defaulting to the upstream
//!   constants. Production callers use [`XaiOAuth::new`].
//! * **`referrer: "pi"`** (`:151`) is sent verbatim. It is an upstream-registered client
//!   identifier paired with [`CLIENT_ID`], not a cyrup-facing brand string, so rebranding it would
//!   change what xAI's authorization server sees.

use super::device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, DeviceCodePoller, poll_oauth_device_code_flow,
};
use super::interaction::{AuthEvent, AuthInteraction};
use super::query::encode_query;
use super::{OAuthError, now_ms, oauth_credential};
use crate::auth::OAuthAuth;
use crate::auth::types::{Credential, ModelAuth};
use crate::error::AuthError;
use cyrup_core::CancelToken;

// ---------------------------------------------------------------------------
// Constants — xai.ts:8-14
// ---------------------------------------------------------------------------

/// `XAI_CLIENT_ID` (`xai.ts:8`).
pub const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
/// `XAI_SCOPE` (`xai.ts:9`). Space-separated; the urlencoded serializer turns the spaces into `+`.
pub const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
/// `XAI_DEVICE_CODE_URL` (`xai.ts:10`).
pub const DEVICE_CODE_URL: &str = "https://auth.x.ai/oauth2/device/code";
/// `XAI_TOKEN_URL` (`xai.ts:11`).
pub const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
/// `REFRESH_SKEW_MS = 5 * 60 * 1000` (`xai.ts:12-13`) — upstream's comment: "Refresh slightly
/// before the reported expiry to avoid using a token that dies mid-request."
pub const REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;
/// `DEFAULT_TOKEN_LIFETIME_SECONDS = 3600` (`xai.ts:14`).
pub const DEFAULT_TOKEN_LIFETIME_SECONDS: f64 = 3600.0;
/// `referrer: "pi"` (`xai.ts:151`) — see this module's divergence note.
pub const REFERRER: &str = "pi";

/// `xai.ts:56`, `:59`.
const UNTRUSTED_URI_MESSAGE: &str = "Untrusted verification URI in xAI OAuth response";
/// `xai.ts:191`.
const DENIED_MESSAGE: &str = "xAI device authorization was denied";
/// `xai.ts:194`.
const EXPIRED_MESSAGE: &str = "xAI device code expired";

// ---------------------------------------------------------------------------
// OAuthHttpResponse — xai.ts:18-22
// ---------------------------------------------------------------------------

/// `OAuthHttpResponse` (`xai.ts:18-22`). `body` is `{}` whenever the payload was not a JSON
/// object, which is what `:86` collapses arrays and scalars to.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub ok: bool,
    pub status: u16,
    pub body: serde_json::Map<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// XaiDeviceCode — xai.ts:24-31
// ---------------------------------------------------------------------------

/// `XaiDeviceCode` (`xai.ts:24-31`).
#[derive(Clone, Debug, PartialEq)]
pub struct XaiDeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub interval_seconds: Option<f64>,
    pub expires_in_seconds: f64,
}

// ---------------------------------------------------------------------------
// Field validators — xai.ts:33-47
// ---------------------------------------------------------------------------

/// `requiredString` (`xai.ts:33-39`): a non-empty JSON string, or
/// `Invalid xAI OAuth response field: {field}`.
pub fn required_string(
    body: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String, OAuthError> {
    match body.get(field).and_then(|v| v.as_str()) {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
        _ => Err(invalid_field(field)),
    }
}

/// `positiveNumber` (`xai.ts:41-47`): a finite JSON number greater than zero, or the same message.
/// A numeric *string* fails, matching `typeof value !== "number"`.
pub fn positive_number(
    body: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<f64, OAuthError> {
    match body.get(field).and_then(|v| v.as_f64()) {
        Some(value) if value.is_finite() && value > 0.0 => Ok(value),
        _ => Err(invalid_field(field)),
    }
}

/// `xai.ts:36`, `:44`.
fn invalid_field(field: &str) -> OAuthError {
    OAuthError::Failed(format!("Invalid xAI OAuth response field: {field}"))
}

// ---------------------------------------------------------------------------
// validateVerificationUri — xai.ts:49-62
// ---------------------------------------------------------------------------

/// `validateVerificationUri` (`xai.ts:49-62`). Upstream's comment states the intent verbatim:
/// "The verification URI is opened in the user's browser; force it to be an https URL so a
/// malicious response cannot make `open` launch something else."
///
/// **Divergence.** Upstream gets `new URL(raw)` and returns `url.href`; this crate has no URL
/// parser in its dependency graph. This function reproduces the parts of the WHATWG algorithm the
/// check actually rests on — the leading/trailing C0-and-space trim, tab/newline removal, the
/// scheme-start state, the special-scheme authority slashes, and the "special scheme needs a
/// non-empty host" rule — and reproduces the `href` serializer only where it is observable here:
/// the scheme and host are ASCII-lowercased, a default `:443` is dropped, and an empty path
/// becomes `/`. Path, query and fragment are otherwise passed through rather than re-encoded, and
/// IDNA/punycode host mapping is not performed. Anything that would make `new URL` *throw* is
/// rejected with upstream's message, and anything whose scheme is not `https` is rejected with the
/// same message — the two behaviours callers depend on.
pub fn validate_verification_uri(raw: &str) -> Result<String, OAuthError> {
    let untrusted = || OAuthError::Failed(UNTRUSTED_URI_MESSAGE.to_string());

    // WHATWG "basic URL parser": strip leading/trailing C0 controls and space, then remove all
    // ASCII tab and newline.
    let trimmed = raw.trim_matches(|c: char| c <= '\u{20}');
    let cleaned: String = trimmed
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();

    // Scheme-start / scheme states.
    let (scheme, rest) = cleaned.split_once(':').ok_or_else(untrusted)?;
    let mut scheme_chars = scheme.chars();
    let first = scheme_chars.next().ok_or_else(untrusted)?;
    if !first.is_ascii_alphabetic()
        || !scheme_chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    {
        return Err(untrusted());
    }
    // `:58` — `url.protocol !== "https:"`. The scheme is lowercased by the parser first.
    if !scheme.eq_ignore_ascii_case("https") {
        return Err(untrusted());
    }

    // "special authority slashes" state: a special scheme skips any run of `/` and `\`.
    let rest = rest.trim_start_matches(['/', '\\']);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);

    // A special scheme with an empty host is a parse failure.
    let (userinfo, host) = match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, authority),
    };
    if host.is_empty() || host.contains(' ') {
        return Err(untrusted());
    }

    // Serializer: lowercase host, drop the default port.
    let host = host.to_ascii_lowercase();
    let host = host.strip_suffix(":443").unwrap_or(&host);

    let mut href = String::from("https://");
    if let Some(userinfo) = userinfo {
        href.push_str(userinfo);
        href.push('@');
    }
    href.push_str(host);
    // An empty path serializes as `/`.
    if tail.is_empty() || tail.starts_with('?') || tail.starts_with('#') {
        href.push('/');
    }
    href.push_str(tail);
    Ok(href)
}

// ---------------------------------------------------------------------------
// postForm — xai.ts:64-98
// ---------------------------------------------------------------------------

/// Race a future against the login-wide abort. Upstream hands `signal` straight to `fetch`
/// (`xai.ts:74`); Rust has no ambient fetch, so the abort is a `select!`. (The same three lines
/// appear in [`super::openrouter`]; they are duplicated rather than shared because neither flow
/// owns the substrate module they would otherwise live in.)
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

/// 1:1 port of `postForm` (`xai.ts:64-98`): an `application/x-www-form-urlencoded` POST whose
/// JSON reply is normalized to an object.
///
/// A transport failure under an aborted signal is `"Login cancelled"` (`:77-79`); a body that will
/// not parse is `xAI OAuth returned invalid JSON (HTTP {status})` (`:91`); a body that parses to
/// something other than an object leaves `body = {}` without failing (`:86`).
pub async fn post_form(
    client: &reqwest::Client,
    url: &str,
    fields: &[(&str, &str)],
    cancel: Option<&CancelToken>,
) -> Result<HttpResponse, OAuthError> {
    // `:73` — `new URLSearchParams(fields)`, in insertion order.
    let form = encode_query(fields.iter().copied());

    let send = client
        .post(url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form)
        .send();

    let response = match with_cancel(send, cancel).await? {
        Ok(response) => response,
        // `:76-81`
        Err(error) => {
            if cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(OAuthError::Cancelled);
            }
            return Err(OAuthError::Failed(error.to_string()));
        }
    };

    let status = response.status();
    // `:84-92` — the body read and the JSON parse share one `catch`.
    let invalid_json = || {
        if cancel.is_some_and(CancelToken::is_cancelled) {
            OAuthError::Cancelled
        } else {
            OAuthError::Failed(format!(
                "xAI OAuth returned invalid JSON (HTTP {})",
                status.as_u16()
            ))
        }
    };
    let text = match with_cancel(response.text(), cancel).await? {
        Ok(text) => text,
        Err(_) => return Err(invalid_json()),
    };
    let body = match serde_json::from_str::<serde_json::Value>(&text) {
        // `:86` — only a non-array object is adopted; anything else becomes `{}`.
        Ok(serde_json::Value::Object(map)) => map,
        Ok(_) => serde_json::Map::new(),
        Err(_) => return Err(invalid_json()),
    };

    // `:93-97`
    Ok(HttpResponse {
        ok: status.is_success(),
        status: status.as_u16(),
        body,
    })
}

// ---------------------------------------------------------------------------
// requestFailure — xai.ts:100-106
// ---------------------------------------------------------------------------

/// 1:1 port of `requestFailure` (`xai.ts:100-106`):
/// `xAI OAuth {action} failed (HTTP {status})[: {error}[: {description}]]`.
///
/// `[error, description].filter(Boolean)` drops absent **and** empty strings, and the survivors are
/// joined with `": "`.
pub fn request_failure(action: &str, response: &HttpResponse) -> OAuthError {
    let error = response.body.get("error").and_then(|v| v.as_str());
    let description = response
        .body
        .get("error_description")
        .and_then(|v| v.as_str());
    let detail = [error, description]
        .into_iter()
        .flatten()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(": ");
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    OAuthError::Failed(format!(
        "xAI OAuth {action} failed (HTTP {}){suffix}",
        response.status
    ))
}

// ---------------------------------------------------------------------------
// parseDeviceCode — xai.ts:108-126
// ---------------------------------------------------------------------------

/// 1:1 port of `parseDeviceCode` (`xai.ts:108-126`).
///
/// Field evaluation order is upstream's, and it is observable: `verification_uri_complete` is
/// validated at `:114-117`, **before** the `device_code`/`user_code` checks in the returned object
/// literal, so a malicious complete-URI is reported even when the payload is also missing
/// mandatory fields.
///
/// Upstream's comment on the interval: "RFC 8628 allows interval 0 (no minimum wait); fall back to
/// the poller's default instead of failing on non-positive or malformed values."
pub fn parse_device_code(
    body: &serde_json::Map<String, serde_json::Value>,
) -> Result<XaiDeviceCode, OAuthError> {
    // `:111-113`
    let interval_seconds = body
        .get("interval")
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite() && *v > 0.0);

    // `:114-117`
    let verification_uri_complete = match body
        .get("verification_uri_complete")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
    {
        Some(raw) => Some(validate_verification_uri(raw)?),
        None => None,
    };

    // `:118-125`
    Ok(XaiDeviceCode {
        device_code: required_string(body, "device_code")?,
        user_code: required_string(body, "user_code")?,
        verification_uri: validate_verification_uri(&required_string(body, "verification_uri")?)?,
        verification_uri_complete,
        interval_seconds,
        expires_in_seconds: positive_number(body, "expires_in")?,
    })
}

// ---------------------------------------------------------------------------
// credentialsFromTokenResponse — xai.ts:128-143
// ---------------------------------------------------------------------------

/// 1:1 port of `credentialsFromTokenResponse` (`xai.ts:128-143`).
///
/// Upstream's comment: "xAI may omit refresh_token on refresh when the token is not rotated." That
/// fallback keys on `=== undefined`, i.e. the key being **absent**; a present `null` still goes
/// through `requiredString` and fails.
pub fn credentials_from_token_response(
    body: &serde_json::Map<String, serde_json::Value>,
    previous_refresh_token: Option<&str>,
) -> Result<Credential, OAuthError> {
    // `:129`
    let access = required_string(body, "access_token")?;

    // `:131-134`
    let refresh = match previous_refresh_token {
        Some(previous) if !previous.is_empty() && !body.contains_key("refresh_token") => {
            previous.to_string()
        }
        _ => required_string(body, "refresh_token")?,
    };

    // `:135-136`
    let expires_in_seconds = if body.contains_key("expires_in") {
        positive_number(body, "expires_in")?
    } else {
        DEFAULT_TOKEN_LIFETIME_SECONDS
    };

    // `:141` — `Date.now() + expiresInSeconds * 1000 - REFRESH_SKEW_MS`, in MILLISECONDS. JS keeps
    // a fractional `expires_in` as a float; the stored deadline is an integer, so it truncates.
    Ok(oauth_credential(
        access,
        refresh,
        now_ms() + (expires_in_seconds * 1000.0) as i64 - REFRESH_SKEW_MS,
    ))
}

// ---------------------------------------------------------------------------
// The token poller — xai.ts:167-197
// ---------------------------------------------------------------------------

/// The `poll` callback of `pollForTokens` (`xai.ts:167-197`), as a
/// [`DeviceCodePoller`] for [`poll_oauth_device_code_flow`].
struct XaiTokenPoller {
    client: reqwest::Client,
    token_url: String,
    device_code: String,
    cancel: Option<CancelToken>,
}

#[async_trait::async_trait]
impl DeviceCodePoller for XaiTokenPoller {
    type Value = Credential;

    async fn poll(&self) -> Result<DeviceCodePollResult<Credential>, OAuthError> {
        // `:168-176`
        let response = post_form(
            &self.client,
            &self.token_url,
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", CLIENT_ID),
                ("device_code", &self.device_code),
            ],
            self.cancel.as_ref(),
        )
        .await?;

        // `:178-180` — a malformed success body throws out of the poll, ending the flow.
        if response.ok {
            return Ok(DeviceCodePollResult::Complete(
                credentials_from_token_response(&response.body, None)?,
            ));
        }

        // `:182-196` — a non-string `error` matches none of the branches and lands on `:196`.
        match response.body.get("error").and_then(|v| v.as_str()) {
            Some("authorization_pending") => Ok(DeviceCodePollResult::Pending),
            Some("slow_down") => Ok(DeviceCodePollResult::SlowDown {
                interval_seconds: response.body.get("interval").and_then(|v| v.as_f64()),
            }),
            Some("access_denied" | "authorization_denied") => Ok(DeviceCodePollResult::Failed {
                message: DENIED_MESSAGE.to_string(),
            }),
            Some("expired_token") => Ok(DeviceCodePollResult::Failed {
                message: EXPIRED_MESSAGE.to_string(),
            }),
            _ => Ok(DeviceCodePollResult::Failed {
                message: request_failure("device token polling", &response).to_string(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// xaiOAuth — xai.ts:229-238
// ---------------------------------------------------------------------------

/// The xAI (Grok / X subscription) OAuth strategy — upstream's `xaiOAuth` (`xai.ts:229-238`).
#[derive(Clone, Debug)]
pub struct XaiOAuth {
    device_code_url: String,
    token_url: String,
}

impl Default for XaiOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl XaiOAuth {
    /// The production strategy — upstream's `:10-11` endpoints.
    pub fn new() -> Self {
        XaiOAuth {
            device_code_url: DEVICE_CODE_URL.to_string(),
            token_url: TOKEN_URL.to_string(),
        }
    }

    /// Point the flow at different endpoints. Exists only because Rust has no ambient `fetch` for
    /// a test to stub the way upstream's would; production code uses [`XaiOAuth::new`].
    pub fn with_endpoints(device_code_url: impl Into<String>, token_url: impl Into<String>) -> Self {
        XaiOAuth {
            device_code_url: device_code_url.into(),
            token_url: token_url.into(),
        }
    }

    /// PROV-047: proxy-aware, per target. `build_client()` consulted neither the ported resolver
    /// nor the `httpProxy` setting, so every OAuth token exchange and silent refresh bypassed a
    /// configured proxy while provider streaming used it.
    async fn client(&self, target_url: &str) -> Result<reqwest::Client, OAuthError> {
        crate::stream::sse::build_client_for(target_url)
            .await
            .map_err(|e| OAuthError::Failed(e.to_string()))
    }

    /// 1:1 port of `requestDeviceCode` (`xai.ts:145-159`).
    pub async fn request_device_code(
        &self,
        cancel: Option<&CancelToken>,
    ) -> Result<XaiDeviceCode, OAuthError> {
        let client = self.client(&self.device_code_url).await?;
        // `:146-154`
        let response = post_form(
            &client,
            &self.device_code_url,
            &[
                ("client_id", CLIENT_ID),
                ("scope", SCOPE),
                ("referrer", REFERRER),
            ],
            cancel,
        )
        .await?;
        // `:155-157`
        if !response.ok {
            return Err(request_failure("device authorization", &response));
        }
        // `:158`
        parse_device_code(&response.body)
    }

    /// 1:1 port of `pollForTokens` (`xai.ts:161-199`).
    pub async fn poll_for_tokens(
        &self,
        device: &XaiDeviceCode,
        cancel: Option<&CancelToken>,
    ) -> Result<Credential, OAuthError> {
        let poller = XaiTokenPoller {
            client: self.client(&self.token_url).await?,
            token_url: self.token_url.clone(),
            device_code: device.device_code.clone(),
            cancel: cancel.cloned(),
        };
        // `:163-166`
        let options = DeviceCodePollOptions {
            interval_seconds: device.interval_seconds,
            expires_in_seconds: Some(device.expires_in_seconds),
            wait_before_first_poll: true,
            cancel: cancel.cloned(),
        };
        poll_oauth_device_code_flow(&options, &poller).await
    }

    /// 1:1 port of `refreshXaiToken` (`xai.ts:213-227`).
    pub async fn refresh_token(
        &self,
        refresh_token: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<Credential, OAuthError> {
        let client = self.client(&self.token_url).await?;
        // `:214-222`
        let response = post_form(
            &client,
            &self.token_url,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", refresh_token),
            ],
            cancel,
        )
        .await?;
        // `:223-225`
        if !response.ok {
            return Err(request_failure("token refresh", &response));
        }
        // `:226` — the previous token is passed so a non-rotating server keeps it.
        credentials_from_token_response(&response.body, Some(refresh_token))
    }

    /// `loginXai` (`xai.ts:201-211`).
    async fn run_login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        let cancel = interaction.cancel().cloned();
        // `:202`
        let device = self.request_device_code(cancel.as_ref()).await?;
        // `:203-209` — `verificationUriComplete ?? verificationUri`.
        interaction.notify(AuthEvent::DeviceCode {
            user_code: device.user_code.clone(),
            verification_uri: device
                .verification_uri_complete
                .clone()
                .unwrap_or_else(|| device.verification_uri.clone()),
            interval_seconds: device.interval_seconds,
            expires_in_seconds: Some(device.expires_in_seconds),
        });
        // `:210`
        self.poll_for_tokens(&device, cancel.as_ref()).await
    }
}

#[async_trait::async_trait]
impl OAuthAuth for XaiOAuth {
    /// `xai.ts:230`
    fn name(&self) -> &str {
        "xAI (Grok/X subscription)"
    }

    /// `isSubscription: true` (pi v0.84.1 `oauth/xai.ts:231`) — SuperGrok / X Premium.
    fn is_subscription(&self) -> bool {
        true
    }

    /// `xai.ts:231`
    fn login_label(&self) -> Option<&str> {
        Some("Sign in with SuperGrok or X Premium")
    }

    /// 1:1 port of `loginXai` (`xai.ts:201-211`, wired at `:232`): request a device code, show it,
    /// then poll until the user finishes in the browser.
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        self.run_login(interaction).await
    }

    /// `xai.ts:233` — `refresh: (credential, signal) => refreshXaiToken(credential.refresh, signal)`.
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        let refresh = match cred {
            Credential::Oauth { refresh, .. } => refresh.as_str(),
            Credential::ApiKey { .. } => {
                return Err(OAuthError::Failed(
                    "xAI OAuth refresh requires an oauth credential".to_string(),
                )
                .into_auth_error("xai"));
            }
        };
        self.refresh_token(refresh, None)
            .await
            .map_err(|e| e.into_auth_error("xai"))
    }

    /// `xai.ts:235-237` — `{ apiKey: credential.access }`.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        match cred {
            Credential::Oauth { access, .. } => Ok(ModelAuth {
                api_key: Some(access.clone()),
                ..Default::default()
            }),
            Credential::ApiKey { .. } => Err(OAuthError::Failed(
                "xAI OAuth toAuth requires an oauth credential".to_string(),
            )
            .into_auth_error("xai")),
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

    // -- upstream-derived constants -----------------------------------------

    #[test]
    fn constants_are_upstream_verbatim() {
        // xai.ts:8-14
        assert_eq!(CLIENT_ID, "b1a00492-073a-47ea-816f-4c329264a828");
        assert_eq!(
            SCOPE,
            "openid profile email offline_access grok-cli:access api:access"
        );
        assert_eq!(DEVICE_CODE_URL, "https://auth.x.ai/oauth2/device/code");
        assert_eq!(TOKEN_URL, "https://auth.x.ai/oauth2/token");
        assert_eq!(REFRESH_SKEW_MS, 300_000);
        assert_eq!(DEFAULT_TOKEN_LIFETIME_SECONDS, 3600.0);
        assert_eq!(REFERRER, "pi");
        // xai.ts:230-231
        let flow = XaiOAuth::new();
        assert_eq!(flow.name(), "xAI (Grok/X subscription)");
        assert_eq!(
            flow.login_label(),
            Some("Sign in with SuperGrok or X Premium")
        );
    }

    fn object(json: &str) -> serde_json::Map<String, serde_json::Value> {
        match serde_json::from_str::<serde_json::Value>(json) {
            Ok(serde_json::Value::Object(map)) => map,
            other => panic!("fixture is not a JSON object: {other:?}"),
        }
    }

    // -- validateVerificationUri, xai.ts:49-62 ------------------------------

    #[test]
    fn https_verification_uris_are_accepted() {
        assert_eq!(
            validate_verification_uri("https://auth.x.ai/device?user_code=ABCD-EFGH").unwrap(),
            "https://auth.x.ai/device?user_code=ABCD-EFGH"
        );
        // `url.href` gives a bare origin an empty path of `/`.
        assert_eq!(
            validate_verification_uri("https://auth.x.ai").unwrap(),
            "https://auth.x.ai/"
        );
        // ...lowercases the scheme and host, and drops the default port.
        assert_eq!(
            validate_verification_uri("HTTPS://Auth.X.AI:443/device").unwrap(),
            "https://auth.x.ai/device"
        );
    }

    #[test]
    fn non_https_verification_uris_are_rejected() {
        // xai.ts:58-60 — the whole point of the check.
        for raw in [
            "http://auth.x.ai/device",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "vscode://malicious/run",
            "ftp://auth.x.ai/device",
        ] {
            assert_eq!(
                validate_verification_uri(raw).unwrap_err().to_string(),
                "Untrusted verification URI in xAI OAuth response",
                "raw={raw}"
            );
        }
    }

    #[test]
    fn unparseable_verification_uris_are_rejected() {
        // xai.ts:53-57 — `new URL` throws.
        for raw in ["", "auth.x.ai/device", "://auth.x.ai", "https://", "1https://x"] {
            assert_eq!(
                validate_verification_uri(raw).unwrap_err().to_string(),
                "Untrusted verification URI in xAI OAuth response",
                "raw={raw}"
            );
        }
    }

    // -- field validators, xai.ts:33-47 -------------------------------------

    #[test]
    fn required_string_rejects_non_strings_and_empties() {
        let body = object(r#"{"ok":"v","empty":"","number":7,"null":null,"array":["v"]}"#);
        assert_eq!(required_string(&body, "ok").unwrap(), "v");
        for field in ["empty", "number", "null", "array", "absent"] {
            assert_eq!(
                required_string(&body, field).unwrap_err().to_string(),
                format!("Invalid xAI OAuth response field: {field}")
            );
        }
    }

    #[test]
    fn positive_number_rejects_non_numbers_and_non_positives() {
        let body = object(r#"{"ok":600,"frac":1.5,"zero":0,"neg":-1,"str":"600","null":null}"#);
        assert_eq!(positive_number(&body, "ok").unwrap(), 600.0);
        assert_eq!(positive_number(&body, "frac").unwrap(), 1.5);
        for field in ["zero", "neg", "str", "null", "absent"] {
            assert_eq!(
                positive_number(&body, field).unwrap_err().to_string(),
                format!("Invalid xAI OAuth response field: {field}")
            );
        }
    }

    // -- requestFailure, xai.ts:100-106 -------------------------------------

    fn response(status: u16, json: &str) -> HttpResponse {
        HttpResponse {
            ok: (200..300).contains(&status),
            status,
            body: object(json),
        }
    }

    #[test]
    fn request_failure_joins_error_and_description() {
        assert_eq!(
            request_failure(
                "device authorization",
                &response(400, r#"{"error":"invalid_client","error_description":"unknown"}"#)
            )
            .to_string(),
            "xAI OAuth device authorization failed (HTTP 400): invalid_client: unknown"
        );
        assert_eq!(
            request_failure("token refresh", &response(401, r#"{"error":"invalid_grant"}"#))
                .to_string(),
            "xAI OAuth token refresh failed (HTTP 401): invalid_grant"
        );
        assert_eq!(
            request_failure(
                "token refresh",
                &response(401, r#"{"error_description":"expired"}"#)
            )
            .to_string(),
            "xAI OAuth token refresh failed (HTTP 401): expired"
        );
        // `filter(Boolean)` drops empty strings, leaving no `: detail` at all.
        assert_eq!(
            request_failure("token refresh", &response(500, r#"{"error":""}"#)).to_string(),
            "xAI OAuth token refresh failed (HTTP 500)"
        );
        assert_eq!(
            request_failure("device token polling", &response(503, "{}")).to_string(),
            "xAI OAuth device token polling failed (HTTP 503)"
        );
    }

    // -- parseDeviceCode, xai.ts:108-126 ------------------------------------

    #[test]
    fn parses_a_full_device_code_response() {
        let device = parse_device_code(&object(
            r#"{
                "device_code":"DC",
                "user_code":"ABCD-EFGH",
                "verification_uri":"https://auth.x.ai/device",
                "verification_uri_complete":"https://auth.x.ai/device?code=ABCD-EFGH",
                "interval":5,
                "expires_in":600
            }"#,
        ))
        .unwrap();
        assert_eq!(
            device,
            XaiDeviceCode {
                device_code: "DC".to_string(),
                user_code: "ABCD-EFGH".to_string(),
                verification_uri: "https://auth.x.ai/device".to_string(),
                verification_uri_complete: Some(
                    "https://auth.x.ai/device?code=ABCD-EFGH".to_string()
                ),
                interval_seconds: Some(5.0),
                expires_in_seconds: 600.0,
            }
        );
    }

    #[test]
    fn interval_zero_falls_back_to_the_pollers_default() {
        // xai.ts:109-113 — "RFC 8628 allows interval 0 ...; fall back to the poller's default
        // instead of failing on non-positive or malformed values."
        for interval in ["0", "-1", r#""5""#, "null"] {
            let device = parse_device_code(&object(&format!(
                r#"{{"device_code":"DC","user_code":"U","verification_uri":"https://auth.x.ai/d","interval":{interval},"expires_in":600}}"#
            )))
            .unwrap();
            assert_eq!(device.interval_seconds, None, "interval={interval}");
        }
        // Mirror: a valid interval IS carried through, so the loop above is not vacuous.
        let device = parse_device_code(&object(
            r#"{"device_code":"DC","user_code":"U","verification_uri":"https://auth.x.ai/d","interval":7,"expires_in":600}"#,
        ))
        .unwrap();
        assert_eq!(device.interval_seconds, Some(7.0));
    }

    #[test]
    fn empty_verification_uri_complete_is_treated_as_absent() {
        // xai.ts:114-117 — `.length > 0` guards the validation.
        let device = parse_device_code(&object(
            r#"{"device_code":"DC","user_code":"U","verification_uri":"https://auth.x.ai/d","verification_uri_complete":"","expires_in":600}"#,
        ))
        .unwrap();
        assert_eq!(device.verification_uri_complete, None);
    }

    #[test]
    fn a_hostile_complete_uri_is_rejected_before_the_required_fields() {
        // xai.ts:114-117 runs before the object literal at `:118-125`, so the URI check wins even
        // though `device_code` is also missing.
        let err = parse_device_code(&object(
            r#"{"verification_uri_complete":"vscode://evil/run","expires_in":600}"#,
        ))
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Untrusted verification URI in xAI OAuth response"
        );
    }

    #[test]
    fn missing_required_device_code_fields_are_reported_by_name() {
        let err = parse_device_code(&object(
            r#"{"user_code":"U","verification_uri":"https://auth.x.ai/d","expires_in":600}"#,
        ))
        .unwrap_err();
        assert_eq!(err.to_string(), "Invalid xAI OAuth response field: device_code");

        let err = parse_device_code(&object(
            r#"{"device_code":"DC","user_code":"U","verification_uri":"https://auth.x.ai/d"}"#,
        ))
        .unwrap_err();
        assert_eq!(err.to_string(), "Invalid xAI OAuth response field: expires_in");

        // xai.ts:121 — `verification_uri` is validated as well as required.
        let err = parse_device_code(&object(
            r#"{"device_code":"DC","user_code":"U","verification_uri":"http://auth.x.ai/d","expires_in":600}"#,
        ))
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Untrusted verification URI in xAI OAuth response"
        );
    }

    // -- credentialsFromTokenResponse, xai.ts:128-143 -----------------------

    fn parts(cred: &Credential) -> (String, String, i64) {
        match cred {
            Credential::Oauth {
                access,
                refresh,
                expires,
                ..
            } => (access.clone(), refresh.clone(), *expires),
            other => panic!("expected oauth credential, got {other:?}"),
        }
    }

    #[test]
    fn token_response_computes_the_skewed_expiry() {
        let before = now_ms();
        let cred = credentials_from_token_response(
            &object(r#"{"access_token":"acc","refresh_token":"ref","expires_in":3600}"#),
            None,
        )
        .unwrap();
        let after = now_ms();
        let (access, refresh, expires) = parts(&cred);
        assert_eq!(access, "acc");
        assert_eq!(refresh, "ref");
        // xai.ts:141 — Date.now() + expires_in*1000 - 5*60*1000, in MILLISECONDS. The literals are
        // upstream's, deliberately NOT `REFRESH_SKEW_MS`: asserting against the constant under
        // test would make the assertion self-fulfilling.
        assert!(expires >= before + 3_600_000 - 300_000, "{expires}");
        assert!(expires <= after + 3_600_000 - 300_000, "{expires}");
    }

    #[test]
    fn absent_expires_in_uses_the_default_lifetime() {
        // xai.ts:135-136
        let before = now_ms();
        let cred = credentials_from_token_response(
            &object(r#"{"access_token":"acc","refresh_token":"ref"}"#),
            None,
        )
        .unwrap();
        let (_, _, expires) = parts(&cred);
        assert!(expires >= before + 3_600_000 - 300_000, "{expires}");
    }

    #[test]
    fn an_omitted_refresh_token_keeps_the_previous_one() {
        // xai.ts:130-134 — "xAI may omit refresh_token on refresh when the token is not rotated."
        let cred = credentials_from_token_response(
            &object(r#"{"access_token":"acc2","expires_in":60}"#),
            Some("old-refresh"),
        )
        .unwrap();
        assert_eq!(parts(&cred).1, "old-refresh");

        // Mirror: when the server DOES rotate, the new token wins over the previous one.
        let cred = credentials_from_token_response(
            &object(r#"{"access_token":"acc2","refresh_token":"new-refresh","expires_in":60}"#),
            Some("old-refresh"),
        )
        .unwrap();
        assert_eq!(parts(&cred).1, "new-refresh");
    }

    #[test]
    fn a_present_null_refresh_token_is_not_undefined() {
        // xai.ts:132 — `body.refresh_token === undefined`. `null` is present, so `requiredString`
        // runs and fails; the previous token is NOT substituted.
        let err = credentials_from_token_response(
            &object(r#"{"access_token":"acc","refresh_token":null,"expires_in":60}"#),
            Some("old-refresh"),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid xAI OAuth response field: refresh_token"
        );
    }

    #[test]
    fn a_missing_refresh_token_with_no_previous_fails() {
        let err = credentials_from_token_response(
            &object(r#"{"access_token":"acc","expires_in":60}"#),
            None,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid xAI OAuth response field: refresh_token"
        );
    }

    // -- a scripted loopback endpoint ---------------------------------------

    /// Serves a fixed list of responses, one per connection, off `127.0.0.1`. Never touches the
    /// network. Every reply carries `Connection: close`, so `reqwest` opens a fresh connection per
    /// request and the scripted order is the request order.
    struct FakeEndpoint {
        url: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl FakeEndpoint {
        fn start(path: &'static str, responses: Vec<(u16, String)>) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&requests);
            std::thread::spawn(move || {
                for (status, body) in responses {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    let mut buf = [0u8; 8192];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let raw = String::from_utf8_lossy(&buf[..n]).to_string();
                    let (head, payload) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
                    if let Ok(mut log) = sink.lock() {
                        log.push((head.to_string(), payload.to_string()));
                    }
                    let reason = if (200..300).contains(&status) {
                        "OK"
                    } else {
                        "Bad Request"
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
                url: format!("http://127.0.0.1:{port}{path}"),
                requests,
            }
        }

        fn one(path: &'static str, status: u16, body: &str) -> Self {
            Self::start(path, vec![(status, body.to_string())])
        }

        fn recorded(&self) -> Vec<(String, String)> {
            self.requests.lock().map(|r| r.clone()).unwrap_or_default()
        }
    }

    fn client() -> reqwest::Client {
        crate::stream::sse::build_client().unwrap()
    }

    // -- postForm / requestDeviceCode wire shape ----------------------------

    #[tokio::test]
    async fn device_code_request_sends_the_upstream_form() {
        let server = FakeEndpoint::one(
            "/oauth2/device/code",
            200,
            r#"{"device_code":"DC","user_code":"U-1","verification_uri":"https://auth.x.ai/device","expires_in":600,"interval":5}"#,
        );
        let flow = XaiOAuth::with_endpoints(&server.url, TOKEN_URL);
        let device = flow.request_device_code(None).await.unwrap();
        assert_eq!(device.user_code, "U-1");

        let recorded = server.recorded();
        let (head, body) = recorded.first().cloned().unwrap();
        assert!(head.starts_with("POST /oauth2/device/code HTTP/1.1"), "{head}");
        // xai.ts:69-72
        let lower = head.to_lowercase();
        assert!(lower.contains("accept: application/json"), "{head}");
        assert!(
            lower.contains("content-type: application/x-www-form-urlencoded"),
            "{head}"
        );
        // xai.ts:148-152, serialized by `new URLSearchParams(fields)`: spaces become `+`, `:`
        // becomes `%3A`, and the field order is the object's insertion order.
        assert_eq!(
            body,
            concat!(
                "client_id=b1a00492-073a-47ea-816f-4c329264a828",
                "&scope=openid+profile+email+offline_access+grok-cli%3Aaccess+api%3Aaccess",
                "&referrer=pi",
            )
        );
    }

    #[tokio::test]
    async fn a_failed_device_authorization_reports_upstream_message() {
        // xai.ts:155-157
        let server = FakeEndpoint::one(
            "/oauth2/device/code",
            400,
            r#"{"error":"invalid_client","error_description":"bad client"}"#,
        );
        let flow = XaiOAuth::with_endpoints(&server.url, TOKEN_URL);
        assert_eq!(
            flow.request_device_code(None).await.unwrap_err().to_string(),
            "xAI OAuth device authorization failed (HTTP 400): invalid_client: bad client"
        );
    }

    #[tokio::test]
    async fn a_non_json_body_reports_the_status() {
        // xai.ts:87-92
        let server = FakeEndpoint::one("/oauth2/device/code", 200, "<html>nope</html>");
        let flow = XaiOAuth::with_endpoints(&server.url, TOKEN_URL);
        assert_eq!(
            flow.request_device_code(None).await.unwrap_err().to_string(),
            "xAI OAuth returned invalid JSON (HTTP 200)"
        );
    }

    #[tokio::test]
    async fn a_json_array_body_is_normalized_to_an_empty_object() {
        // xai.ts:86 — an array is not adopted, so the required-field check reports the miss rather
        // than a JSON error.
        let server = FakeEndpoint::one("/oauth2/device/code", 200, r#"["nope"]"#);
        let flow = XaiOAuth::with_endpoints(&server.url, TOKEN_URL);
        assert_eq!(
            flow.request_device_code(None).await.unwrap_err().to_string(),
            "Invalid xAI OAuth response field: device_code"
        );
    }

    // -- the poll callback, xai.ts:167-197 ----------------------------------

    async fn poll_once(status: u16, body: &str) -> DeviceCodePollResult<Credential> {
        let server = FakeEndpoint::one("/oauth2/token", status, body);
        let poller = XaiTokenPoller {
            client: client(),
            token_url: server.url.clone(),
            device_code: "DC".to_string(),
            cancel: None,
        };
        let result = poller.poll().await.unwrap();
        // xai.ts:170-174 — the grant type is the RFC 8628 URN, and the form carries no scope.
        let recorded = server.recorded();
        let (_, form) = recorded.first().cloned().unwrap();
        assert_eq!(
            form,
            concat!(
                "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
                "&client_id=b1a00492-073a-47ea-816f-4c329264a828",
                "&device_code=DC",
            )
        );
        result
    }

    /// `DeviceCodePollResult` is only `PartialEq` when its payload is, and `Credential` is not, so
    /// each `{ status, ... }` upstream returns is compared as its rendered form.
    fn describe(result: &DeviceCodePollResult<Credential>) -> String {
        match result {
            DeviceCodePollResult::Pending => "pending".to_string(),
            DeviceCodePollResult::SlowDown { interval_seconds } => {
                format!("slow_down:{interval_seconds:?}")
            }
            DeviceCodePollResult::Failed { message } => format!("failed:{message}"),
            DeviceCodePollResult::Complete(cred) => format!("complete:{}", parts(cred).0),
        }
    }

    #[tokio::test]
    async fn poll_maps_every_upstream_status() {
        // xai.ts:178-180
        assert_eq!(
            describe(
                &poll_once(200, r#"{"access_token":"a","refresh_token":"r","expires_in":60}"#).await
            ),
            "complete:a"
        );
        // xai.ts:183-185
        assert_eq!(
            describe(&poll_once(400, r#"{"error":"authorization_pending"}"#).await),
            "pending"
        );
        // xai.ts:186-189
        assert_eq!(
            describe(&poll_once(400, r#"{"error":"slow_down","interval":10}"#).await),
            "slow_down:Some(10.0)"
        );
        assert_eq!(
            describe(&poll_once(400, r#"{"error":"slow_down"}"#).await),
            "slow_down:None"
        );
        // A non-numeric `interval` is `undefined` (`:188`).
        assert_eq!(
            describe(&poll_once(400, r#"{"error":"slow_down","interval":"10"}"#).await),
            "slow_down:None"
        );
        // xai.ts:190-192 — both spellings map to the same message.
        for error in ["access_denied", "authorization_denied"] {
            assert_eq!(
                describe(&poll_once(400, &format!(r#"{{"error":"{error}"}}"#)).await),
                "failed:xAI device authorization was denied",
                "error={error}"
            );
        }
        // xai.ts:193-195
        assert_eq!(
            describe(&poll_once(400, r#"{"error":"expired_token"}"#).await),
            "failed:xAI device code expired"
        );
        // xai.ts:196 — anything else, including a non-string `error`.
        assert_eq!(
            describe(&poll_once(500, r#"{"error":"server_error","error_description":"boom"}"#).await),
            "failed:xAI OAuth device token polling failed (HTTP 500): server_error: boom"
        );
        assert_eq!(
            describe(&poll_once(500, r#"{"error":42}"#).await),
            "failed:xAI OAuth device token polling failed (HTTP 500)"
        );
    }

    #[tokio::test]
    async fn a_malformed_success_body_ends_the_poll_with_an_error() {
        // xai.ts:179 — `credentialsFromTokenResponse` throws out of the poll rather than returning
        // a poll status.
        let server = FakeEndpoint::one("/oauth2/token", 200, r#"{"refresh_token":"r"}"#);
        let poller = XaiTokenPoller {
            client: client(),
            token_url: server.url.clone(),
            device_code: "DC".to_string(),
            cancel: None,
        };
        assert_eq!(
            poller.poll().await.unwrap_err().to_string(),
            "Invalid xAI OAuth response field: access_token"
        );
    }

    // -- refreshXaiToken, xai.ts:213-227 ------------------------------------

    #[tokio::test]
    async fn refresh_sends_the_upstream_form_and_keeps_an_unrotated_token() {
        let server = FakeEndpoint::one(
            "/oauth2/token",
            200,
            r#"{"access_token":"fresh","expires_in":1800}"#,
        );
        let flow = XaiOAuth::with_endpoints(DEVICE_CODE_URL, &server.url);
        let cred = flow.refresh_token("stored-refresh", None).await.unwrap();

        let recorded = server.recorded();
        let (_, form) = recorded.first().cloned().unwrap();
        // xai.ts:216-220
        assert_eq!(
            form,
            concat!(
                "grant_type=refresh_token",
                "&client_id=b1a00492-073a-47ea-816f-4c329264a828",
                "&refresh_token=stored-refresh",
            )
        );
        // xai.ts:226 — the previous token is passed in, so an omitted one is preserved.
        let (access, refresh, _) = parts(&cred);
        assert_eq!(access, "fresh");
        assert_eq!(refresh, "stored-refresh");
    }

    #[tokio::test]
    async fn refresh_reports_upstream_failure_message() {
        // xai.ts:223-225
        let server = FakeEndpoint::one("/oauth2/token", 401, r#"{"error":"invalid_grant"}"#);
        let flow = XaiOAuth::with_endpoints(DEVICE_CODE_URL, &server.url);
        assert_eq!(
            flow.refresh_token("dead", None).await.unwrap_err().to_string(),
            "xAI OAuth token refresh failed (HTTP 401): invalid_grant"
        );
    }

    #[tokio::test]
    async fn refresh_through_the_trait_rejects_an_api_key_credential() {
        let flow = XaiOAuth::new();
        let err = flow
            .refresh(&Credential::ApiKey {
                key: Some("sk".into()),
                env: None,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code(), "oauth");
    }

    // -- the whole login ----------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn login_shows_the_device_code_then_polls_to_a_credential() {
        let device_server = FakeEndpoint::one(
            "/oauth2/device/code",
            200,
            r#"{"device_code":"DC-9","user_code":"WXYZ-1234","verification_uri":"https://auth.x.ai/device","verification_uri_complete":"https://auth.x.ai/device?code=WXYZ-1234","interval":1,"expires_in":600}"#,
        );
        // `waitBeforeFirstPoll` (`xai.ts:165`) means the first poll happens one interval in; the
        // pending reply proves the loop keeps going rather than failing on a non-2xx.
        let token_server = FakeEndpoint::start(
            "/oauth2/token",
            vec![
                (400, r#"{"error":"authorization_pending"}"#.to_string()),
                (
                    200,
                    r#"{"access_token":"xai-access","refresh_token":"xai-refresh","expires_in":7200}"#
                        .to_string(),
                ),
            ],
        );

        let flow = XaiOAuth::with_endpoints(&device_server.url, &token_server.url);
        let interaction = ScriptedInteraction::new(Vec::new());
        let cred = flow.login(&interaction).await.expect("login succeeds");

        // xai.ts:203-209 — the COMPLETE uri is what the user is shown when present.
        assert_eq!(
            interaction.events(),
            vec![AuthEvent::DeviceCode {
                user_code: "WXYZ-1234".to_string(),
                verification_uri: "https://auth.x.ai/device?code=WXYZ-1234".to_string(),
                interval_seconds: Some(1.0),
                expires_in_seconds: Some(600.0),
            }]
        );

        let (access, refresh, _) = parts(&cred);
        assert_eq!(access, "xai-access");
        assert_eq!(refresh, "xai-refresh");
        assert_eq!(token_server.recorded().len(), 2);

        // xai.ts:235-237
        let auth = flow.to_auth(&cred).await.unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("xai-access"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn login_falls_back_to_the_plain_verification_uri() {
        let device_server = FakeEndpoint::one(
            "/oauth2/device/code",
            200,
            r#"{"device_code":"DC","user_code":"U","verification_uri":"https://auth.x.ai/device","interval":1,"expires_in":600}"#,
        );
        let token_server = FakeEndpoint::one(
            "/oauth2/token",
            400,
            r#"{"error":"access_denied"}"#,
        );
        let flow = XaiOAuth::with_endpoints(&device_server.url, &token_server.url);
        let interaction = ScriptedInteraction::new(Vec::new());
        let err = flow.login(&interaction).await.unwrap_err();

        // xai.ts:206 — `verificationUriComplete ?? verificationUri`.
        assert_eq!(
            interaction.events(),
            vec![AuthEvent::DeviceCode {
                user_code: "U".to_string(),
                verification_uri: "https://auth.x.ai/device".to_string(),
                interval_seconds: Some(1.0),
                expires_in_seconds: Some(600.0),
            }]
        );
        // xai.ts:191, surfaced by `device-code.ts:73`.
        assert_eq!(err.to_string(), "xAI device authorization was denied");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_aborted_login_reports_login_cancelled() {
        // xai.ts:77-79
        let device_server = FakeEndpoint::one(
            "/oauth2/device/code",
            200,
            r#"{"device_code":"DC","user_code":"U","verification_uri":"https://auth.x.ai/device","interval":1,"expires_in":600}"#,
        );
        let token_server = FakeEndpoint::one("/oauth2/token", 400, r#"{"error":"authorization_pending"}"#);
        let flow = XaiOAuth::with_endpoints(&device_server.url, &token_server.url);
        let token = CancelToken::new();
        token.cancel();
        let interaction = ScriptedInteraction::new(Vec::new()).with_cancel(token);
        assert_eq!(
            flow.login(&interaction).await.unwrap_err().to_string(),
            "Login cancelled"
        );
    }
}
