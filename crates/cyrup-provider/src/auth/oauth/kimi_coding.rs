//! Kimi Code (subscription) OAuth flow — 1:1 port of pi v0.83.0
//! `packages/ai/src/auth/oauth/kimi-coding.ts` (302 lines).
//!
//! An RFC 8628 device authorization grant against `https://auth.kimi.com` with JSON responses. The
//! access token authenticates requests to `https://api.kimi.com/coding` as an
//! `Authorization: Bearer` header (`kimi-coding.ts:1-7`).
//!
//! ## Provenance
//!
//! | this module | `kimi-coding.ts` |
//! |---|---|
//! | [`CLIENT_ID`] / [`DEFAULT_OAUTH_HOST`] and the four tuning constants | `:13-18` |
//! | [`KimiCodingOAuth::oauth_host`] | `getOauthHost`, `:35-38` |
//! | [`read_json`] | `readJson`, `:48-55` |
//! | [`trusted_http_url`] | `trustedHttpUrl`, `:57-67` |
//! | [`DeviceAuthorization`] / [`parse_device_authorization`] | `startDeviceAuthorization`'s parse half, `:85-116` |
//! | [`KimiCodingOAuth::start_device_authorization`] | `startDeviceAuthorization`, `:69-117` |
//! | [`parse_token_response`] | `parseTokenResponse`, `:119-139` |
//! | [`classify_device_token_reply`] / [`TokenPoller`] | `pollForToken`, `:141-207` |
//! | [`KimiCodingOAuth::refresh_token`] | `refreshToken`, `:217-272` |
//! | [`KimiCodingOAuth::run_login`] | `loginKimiCoding`, `:274-286` |
//! | `impl OAuthAuth for KimiCodingOAuth` | `kimiCodingOAuth`, `:288-302` |
//!
//! ## Mechanism divergences (Rust forces these; behaviour is unchanged)
//!
//! * **No ambient `fetch`.** Requests go through `reqwest` built by
//!   [`crate::stream::sse::build_client_for_target`], so the OAuth host honours the same
//!   `HTTP(S)_PROXY`/`NO_PROXY` policy as provider traffic. `AbortSignal.timeout(30_000)`
//!   (`:40-42`) becomes [`reqwest::RequestBuilder::timeout`].
//! * **`AbortSignal` is [`CancelToken`]** (arch-00 §3.2). `AbortSignal.any([...])` (`:41`) is a
//!   `select!` between the request and [`CancelToken::cancelled`]; an aborted request surfaces as
//!   [`OAuthError::Cancelled`], whose message is upstream's `CANCEL_MESSAGE`. (Upstream would
//!   surface the platform's `AbortError`, whose text is not a stable contract, so the typed cancel
//!   is the closer equivalent for callers.)
//! * **No ambient `URL`.** [`trusted_http_url`] implements `new URL(...).href` restricted to
//!   http(s) — see its doc for the normalization it does and does not model. Upstream keeps its
//!   own copy of this helper too (`kimi-coding.ts:57-67` vs `github-copilot.ts:174-184`), so the
//!   sibling copy in [`super::github_copilot`] mirrors an upstream duplication rather than
//!   introducing one.
//! * **No `signal` on `OAuthAuth::refresh`.** Upstream's `refresh(credential, signal)` (`:294`)
//!   takes an abort signal that [`crate::auth::OAuthAuth::refresh`] has no parameter for, so the
//!   trait method calls [`KimiCodingOAuth::refresh_token`] with `None` and callers that hold a
//!   token use that method directly.
//! * **`JSON.stringify` key order.** Two error messages embed the raw response object
//!   (`:98`, `:132`, `:268`). `serde_json` renders an object's keys sorted, because this workspace
//!   does not enable its `preserve_order` feature, whereas `JSON.stringify` preserves insertion
//!   order. The message content is otherwise identical.
//! * **Errors are typed.** Upstream `throw new Error(msg)`; here the same message is the `Display`
//!   of an [`OAuthError`], folded into the crate taxonomy at the trait boundary (func-01 R-01-017).

use super::OAuthError;
use super::device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, DeviceCodePoller, poll_oauth_device_code_flow,
};
use super::interaction::{AuthEvent, AuthInteraction};
use super::query::encode_query;
use crate::auth::OAuthAuth;
use crate::auth::types::{AuthContext, Credential, EnvAuthContext, ModelAuth};
use crate::error::AuthError;
use cyrup_core::CancelToken;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants — kimi-coding.ts:13-18
// ---------------------------------------------------------------------------

/// `kimi-coding.ts:13`. A public device-flow client id; RFC 8628 device clients have no secret.
pub const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
/// `kimi-coding.ts:14`.
pub const DEFAULT_OAUTH_HOST: &str = "https://auth.kimi.com";
/// `kimi-coding.ts:15` — the `expires_in` fallback when the server omits one.
const DEVICE_CODE_TIMEOUT_SECONDS: f64 = 15.0 * 60.0;
/// `kimi-coding.ts:16` — the `interval` fallback (RFC 8628 §3.2).
const DEFAULT_POLL_INTERVAL_SECONDS: f64 = 5.0;
/// `kimi-coding.ts:17`.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// `kimi-coding.ts:18` — four attempts in total (`attempt <= REFRESH_MAX_RETRIES`, `:223`).
const REFRESH_MAX_RETRIES: u32 = 3;

/// The scoped env override, checked first (`kimi-coding.ts:36`).
pub const OAUTH_HOST_ENV: &str = "KIMI_CODE_OAUTH_HOST";
/// The legacy override, checked second (`kimi-coding.ts:36`).
pub const OAUTH_HOST_ENV_LEGACY: &str = "KIMI_OAUTH_HOST";

/// `kimi-coding.ts:70` — RFC 8628 §3.1.
const DEVICE_AUTHORIZATION_PATH: &str = "/api/oauth/device_authorization";
/// `kimi-coding.ts:152`, `:233` — RFC 8628 §3.4 and RFC 6749 §6 share this endpoint.
const TOKEN_PATH: &str = "/api/oauth/token";
/// `kimi-coding.ts:161`.
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The provider these failures are attributed to (`providers/all.rs:302`).
pub const KIMI_CODING_PROVIDER_ID: &str = "kimi-coding";

// ---------------------------------------------------------------------------
// readJson / trustedHttpUrl — kimi-coding.ts:48-67
// ---------------------------------------------------------------------------

/// `readJson` (`kimi-coding.ts:48-55`): parse the body, keeping it only when it is truthy and
/// `typeof === "object"`. That admits arrays as well as objects (JS `typeof [] === "object"`) and
/// rejects `null`, numbers, strings and booleans; a parse failure is `null`.
pub fn read_json(body: &str) -> Option<Value> {
    serde_json::from_str::<Value>(body)
        .ok()
        .filter(|v| v.is_object() || v.is_array())
}

/// `trustedHttpUrl` (`kimi-coding.ts:57-67`): `new URL(value).href`, but only for `http:`/`https:`.
/// The verification URI is opened in the user's browser, so a `file:`/`javascript:` URI from a
/// spoofed device endpoint must not survive this function.
///
/// Rust has no WHATWG URL parser here (this crate carries no `url` dependency), so `.href`
/// normalization covers the subset these endpoints can produce: the scheme and host are
/// ASCII-lowercased, WHATWG's "special authority slashes" leniency is honoured
/// (`new URL("https:/x").href === "https://x/"`), and an empty path becomes `/`. Percent-encoding
/// normalization, IDNA/punycode and default-port elision are not modelled; Kimi returns
/// already-normalized ASCII origins, on which none of the three would change anything.
///
/// This duplicates [`super::github_copilot::parse_http_href`] because upstream duplicates it too
/// (`kimi-coding.ts:57-67` and `github-copilot.ts:174-184` are separate copies of the same guard),
/// and because neither flow module may depend on the other's registration.
pub fn trusted_http_url(value: &str) -> Option<String> {
    // `typeof value !== "string" || !value` → `new URL("")` never runs (`:59`).
    if value.is_empty() {
        return None;
    }
    let (scheme, rest) = value.split_once(':')?;
    let mut scheme_chars = scheme.chars();
    let first = scheme_chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !scheme_chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    // `:62`
    if scheme != "http" && scheme != "https" {
        return None;
    }

    let rest = rest.trim_start_matches(['/', '\\']);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest.get(..authority_end).unwrap_or("");
    let tail = rest.get(authority_end..).unwrap_or("");

    let (userinfo, host_port) = match authority.rfind('@') {
        Some(idx) => (
            authority.get(..idx).unwrap_or(""),
            authority.get(idx + 1..).unwrap_or(""),
        ),
        None => ("", authority),
    };
    if host_port.starts_with('[') && !host_port.contains(']') {
        return None; // an unterminated IPv6 literal throws
    }
    let host_only = if host_port.starts_with('[') {
        host_port
    } else {
        match host_port.find(':') {
            Some(idx) => host_port.get(..idx).unwrap_or(""),
            None => host_port,
        }
    };
    if host_only.is_empty() {
        return None; // a special scheme with no host throws
    }

    let authority_out = if userinfo.is_empty() {
        host_port.to_ascii_lowercase()
    } else {
        format!("{userinfo}@{}", host_port.to_ascii_lowercase())
    };
    let tail_out = if tail.is_empty() || tail.starts_with('?') || tail.starts_with('#') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    Some(format!("{scheme}://{authority_out}{tail_out}"))
}

/// `JSON.stringify(json)` for the two messages that embed the raw response (`:98`, `:132`, `:268`).
/// A body that [`read_json`] rejected is `null`, exactly as upstream's `readJson` returns.
fn stringify(json: Option<&Value>) -> String {
    serde_json::to_string(json.unwrap_or(&Value::Null)).unwrap_or_else(|_| "null".to_string())
}

// ---------------------------------------------------------------------------
// DeviceAuthorization — kimi-coding.ts:20-27, :85-116
// ---------------------------------------------------------------------------

/// The parsed `device_authorization` payload (`DeviceAuthorization`, `kimi-coding.ts:20-27`).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// The URI actually shown to the user (`:280`) — it embeds the user code.
    pub verification_uri_complete: String,
    /// Already defaulted to [`DEFAULT_POLL_INTERVAL_SECONDS`] when absent or non-positive (`:108`).
    pub interval_seconds: f64,
    /// Already defaulted to [`DEVICE_CODE_TIMEOUT_SECONDS`] when absent or non-positive (`:112`).
    pub expires_in_seconds: f64,
}

/// `startDeviceAuthorization`'s parse half (`kimi-coding.ts:85-116`), split out so every guard is
/// assertable without a socket.
pub fn parse_device_authorization(json: Option<&Value>) -> Result<DeviceAuthorization, OAuthError> {
    let invalid = || {
        OAuthError::Failed(format!(
            "Invalid Kimi Code device authorization response: {}",
            stringify(json)
        ))
    };

    // `:86-99`. Both URIs must be strings AND pass the http(s) guard.
    let get = |key: &str| json.and_then(|j| j.get(key));
    let device_code = get("device_code").and_then(Value::as_str);
    let user_code = get("user_code").and_then(Value::as_str);
    let verification_uri = get("verification_uri").and_then(Value::as_str);
    let verification_uri_complete = get("verification_uri_complete").and_then(Value::as_str);
    let (
        Some(device_code),
        Some(user_code),
        Some(verification_uri),
        Some(verification_uri_complete),
    ) = (
        device_code,
        user_code,
        verification_uri,
        verification_uri_complete,
    )
    else {
        return Err(invalid());
    };
    if trusted_http_url(verification_uri_complete).is_none()
        || trusted_http_url(verification_uri).is_none()
    {
        return Err(invalid());
    }

    // `:101-116`. Note the *raw* strings are kept — upstream returns `deviceCode`/`verificationUri`
    // from the destructured payload, not `trustedHttpUrl`'s normalized `href`.
    let interval_seconds = get("interval")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n > 0.0)
        .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS);
    let expires_in_seconds = get("expires_in")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n > 0.0)
        .unwrap_or(DEVICE_CODE_TIMEOUT_SECONDS);

    Ok(DeviceAuthorization {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        verification_uri: verification_uri.to_string(),
        verification_uri_complete: verification_uri_complete.to_string(),
        interval_seconds,
        expires_in_seconds,
    })
}

// ---------------------------------------------------------------------------
// parseTokenResponse — kimi-coding.ts:29-33, :119-139
// ---------------------------------------------------------------------------

/// `TokenResponse` (`kimi-coding.ts:29-33`). `expires` is a Unix-millisecond deadline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenResponse {
    pub access: String,
    pub refresh: String,
    pub expires: i64,
}

impl TokenResponse {
    /// `{ type: "oauth", access, refresh, expires }` (`:285`, `:296`).
    fn into_credential(self) -> Credential {
        super::oauth_credential(self.access, self.refresh, self.expires)
    }
}

/// `parseTokenResponse` (`kimi-coding.ts:119-139`). Every field is required, non-empty and — for
/// `expires_in` — a finite positive number; `operation` is spliced into the failure message.
///
/// `now_ms` is `Date.now()` (`:137`), injected so the deadline arithmetic is assertable.
pub fn parse_token_response(
    json: Option<&Value>,
    operation: &str,
    now_ms: i64,
) -> Result<TokenResponse, OAuthError> {
    let get = |key: &str| json.and_then(|j| j.get(key));
    let access = get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let refresh = get("refresh_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let expires_in = get("expires_in")
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite() && *n > 0.0);

    let (Some(access), Some(refresh), Some(expires_in)) = (access, refresh, expires_in) else {
        return Err(OAuthError::Failed(format!(
            "Kimi Code token {operation} response missing fields: {}",
            stringify(json)
        )));
    };
    Ok(TokenResponse {
        access: access.to_string(),
        refresh: refresh.to_string(),
        // `Date.now() + expiresIn * 1000` (`:137`) — no safety skew, unlike Anthropic/Copilot.
        expires: now_ms + (expires_in * 1000.0) as i64,
    })
}

/// Map one device-token reply onto a poll result (`kimi-coding.ts:166-204`), split out so every
/// branch is assertable without a socket. `status` is the HTTP status; `json` is [`read_json`]'s
/// output; `text` is the raw body, used only by the 5xx branch.
pub fn classify_device_token_reply(
    status: u16,
    json: Option<&Value>,
    text: &str,
    now_ms: i64,
) -> DeviceCodePollResult<TokenResponse> {
    // `:166-172` — a 5xx never reaches the JSON branches.
    if status >= 500 {
        let suffix = if text.is_empty() {
            String::new()
        } else {
            format!(": {text}")
        };
        return DeviceCodePollResult::Failed {
            message: format!("Kimi Code device token request failed with status {status}{suffix}"),
        };
    }

    let get = |key: &str| json.and_then(|j| j.get(key));
    let ok = (200..300).contains(&status);

    // `:174-181`
    if ok && get("access_token").and_then(Value::as_str).is_some() {
        return match parse_token_response(json, "poll", now_ms) {
            Ok(token) => DeviceCodePollResult::Complete(token),
            Err(error) => DeviceCodePollResult::Failed {
                message: error.to_string(),
            },
        };
    }

    // `:183-184`
    let error = get("error").and_then(Value::as_str);
    let description = get("error_description")
        .and_then(Value::as_str)
        .map(|d| format!(": {d}"))
        .unwrap_or_default();

    match error {
        // `:185-187`
        Some("authorization_pending") => DeviceCodePollResult::Pending,
        // `:188-194`
        Some("slow_down") => DeviceCodePollResult::SlowDown {
            interval_seconds: get("interval").and_then(Value::as_f64).filter(|n| *n > 0.0),
        },
        // `:195-197`
        Some("expired_token") => DeviceCodePollResult::Failed {
            message: "Kimi Code device authorization expired. Please restart login.".to_string(),
        },
        // `:198-200`
        Some("access_denied") => DeviceCodePollResult::Failed {
            message: "Kimi Code login was denied.".to_string(),
        },
        // `:201-204` — the `error` suffix appears only when `error` is a string.
        other => DeviceCodePollResult::Failed {
            message: format!(
                "Kimi Code device token request failed (status {status}){}",
                other
                    .map(|e| format!(": {e}{description}"))
                    .unwrap_or_default()
            ),
        },
    }
}

// ---------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------

/// One completed HTTP round trip: the status plus the raw body. Upstream reads the body at most
/// once per branch (`response.text()` or `response.json()`); reading it eagerly here is equivalent
/// and lets the 5xx branch and the JSON branch share one read.
#[derive(Clone, Debug)]
struct HttpReply {
    status: u16,
    text: String,
}

impl HttpReply {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
    fn json(&self) -> Option<Value> {
        read_json(&self.text)
    }
}

/// Kimi Code's OAuth strategy — upstream's `kimiCodingOAuth` (`kimi-coding.ts:288-302`).
pub struct KimiCodingOAuth {
    /// Ambient context for the `KIMI_*_OAUTH_HOST` overrides and for `HTTP(S)_PROXY`/`NO_PROXY`
    /// resolution, so the OAuth host honours the same proxy policy as provider traffic.
    auth_ctx: Arc<dyn AuthContext>,
}

impl Default for KimiCodingOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl KimiCodingOAuth {
    /// The production strategy: the real process environment.
    pub fn new() -> Self {
        Self {
            auth_ctx: Arc::new(EnvAuthContext),
        }
    }

    /// Override the ambient env/proxy source. This is also the seam the tests use to point the
    /// flow at a loopback listener, because [`Self::oauth_host`] reads the endpoint from the
    /// environment exactly as upstream does — there is no separate test-only endpoint field.
    pub fn with_auth_context(ctx: Arc<dyn AuthContext>) -> Self {
        Self { auth_ctx: ctx }
    }

    /// `getOauthHost` (`kimi-coding.ts:35-38`): `KIMI_CODE_OAUTH_HOST` wins, then
    /// `KIMI_OAUTH_HOST`, then [`DEFAULT_OAUTH_HOST`]; trailing slashes are stripped.
    ///
    /// Empty values fall through, because upstream chains them with `||`;
    /// [`crate::env_api_keys::get_provider_env_value`] already returns `None` for an empty value.
    pub async fn oauth_host(&self) -> String {
        let mut host: Option<String> = None;
        for name in [OAUTH_HOST_ENV, OAUTH_HOST_ENV_LEGACY] {
            if host.is_none() {
                host =
                    crate::env_api_keys::get_provider_env_value(name, self.auth_ctx.as_ref(), None)
                        .await;
            }
        }
        let host = host.unwrap_or_else(|| DEFAULT_OAUTH_HOST.to_string());
        // `.replace(/\/+$/, "")` (`:37`).
        host.trim_end_matches('/').to_string()
    }

    /// Issue one request, racing it against the login's cancel token (`requestSignal`, `:40-42`).
    async fn send(
        &self,
        url: &str,
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
        // `:72-77` / `:153-163` / `:234-245` — the three POSTs share these headers.
        let request = client
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .header("Accept", "application/json")
            .timeout(REQUEST_TIMEOUT)
            .body(body);

        let issue = async {
            let response = request
                .send()
                .await
                .map_err(|e| OAuthError::Failed(e.to_string()))?;
            let status = response.status().as_u16();
            // `await response.text().catch(() => "")` (`:81`, `:167`).
            let text = response.text().await.unwrap_or_default();
            Ok(HttpReply { status, text })
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

    /// `startDeviceAuthorization` (`kimi-coding.ts:69-117`).
    pub async fn start_device_authorization(
        &self,
        oauth_host: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<DeviceAuthorization, OAuthError> {
        let url = format!("{oauth_host}{DEVICE_AUTHORIZATION_PATH}");
        // `:76`
        let body = encode_query([("client_id", CLIENT_ID)]);
        let reply = self.send(&url, body, cancel).await?;

        // `:80-83`
        if !reply.ok() {
            let suffix = if reply.text.is_empty() {
                String::new()
            } else {
                format!(": {}", reply.text)
            };
            return Err(OAuthError::Failed(format!(
                "Kimi Code device authorization failed with status {}{suffix}",
                reply.status
            )));
        }
        parse_device_authorization(reply.json().as_ref())
    }

    /// `pollForToken` (`kimi-coding.ts:141-207`).
    pub async fn poll_for_token(
        &self,
        oauth_host: &str,
        device: &DeviceAuthorization,
        cancel: Option<CancelToken>,
    ) -> Result<TokenResponse, OAuthError> {
        let options = DeviceCodePollOptions {
            // `:147-149`
            interval_seconds: Some(device.interval_seconds),
            expires_in_seconds: Some(device.expires_in_seconds),
            wait_before_first_poll: true,
            cancel: cancel.clone(),
        };
        let poller = TokenPoller {
            flow: self,
            url: format!("{oauth_host}{TOKEN_PATH}"),
            device_code: device.device_code.clone(),
            cancel,
        };
        poll_oauth_device_code_flow(&options, &poller).await
    }

    /// `refreshToken` (`kimi-coding.ts:217-272`): up to [`REFRESH_MAX_RETRIES`] retries with
    /// 1 s / 2 s / 4 s backoff, retrying only 429 and 5xx (and transport failures); 401, 403 and
    /// `invalid_grant` are terminal because the stored credential is dead.
    pub async fn refresh_token(
        &self,
        oauth_host: &str,
        refresh_token: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<TokenResponse, OAuthError> {
        let url = format!("{oauth_host}{TOKEN_PATH}");
        let mut last_error: Option<OAuthError> = None;

        // `:223` — `attempt <= REFRESH_MAX_RETRIES`, i.e. four attempts.
        for attempt in 0..=REFRESH_MAX_RETRIES {
            // `:224-226` — `1000 * 2 ** (attempt - 1)`.
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(1000u64 << (attempt - 1))).await;
            }
            // `:227-229`
            if cancel.is_some_and(CancelToken::is_cancelled) {
                return Err(OAuthError::Failed(
                    "Kimi Code token refresh aborted".to_string(),
                ));
            }

            // `:239-243`
            let body = encode_query([
                ("client_id", CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ]);
            // `:246-249` — a transport failure is remembered and retried.
            let reply = match self.send(&url, body, cancel).await {
                Ok(reply) => reply,
                Err(OAuthError::Cancelled) => return Err(OAuthError::Cancelled),
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };

            let json = reply.json();
            // `:251-254`
            if reply.ok() {
                return parse_token_response(json.as_ref(), "refresh", super::now_ms());
            }

            // `:256-260`
            let invalid_grant = json
                .as_ref()
                .and_then(|j| j.get("error"))
                .and_then(Value::as_str)
                == Some("invalid_grant");
            if reply.status == 401 || reply.status == 403 || invalid_grant {
                let description = json
                    .as_ref()
                    .and_then(|j| j.get("error_description"))
                    .and_then(Value::as_str)
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default();
                return Err(OAuthError::Failed(format!(
                    "Kimi Code token refresh unauthorized (status {}){description}",
                    reply.status
                )));
            }

            // `:262-265`
            let retryable = reply.status == 429 || reply.status >= 500;
            if retryable && attempt < REFRESH_MAX_RETRIES {
                last_error = Some(OAuthError::Failed(format!(
                    "Kimi Code token refresh failed with status {}",
                    reply.status
                )));
                continue;
            }

            // `:267-268`. `JSON.stringify(null)` is the truthy string `"null"`, so the suffix is
            // present even when the body was unparseable — that is upstream's behaviour, not a
            // transcription slip.
            let text = stringify(json.as_ref());
            let suffix = if text.is_empty() {
                String::new()
            } else {
                format!(": {text}")
            };
            return Err(OAuthError::Failed(format!(
                "Kimi Code token refresh failed with status {}{suffix}",
                reply.status
            )));
        }

        // `:271`
        Err(last_error
            .unwrap_or_else(|| OAuthError::Failed("Kimi Code token refresh failed".to_string())))
    }

    /// `loginKimiCoding` (`kimi-coding.ts:274-286`).
    async fn run_login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        let oauth_host = self.oauth_host().await;
        let cancel = interaction.cancel().cloned();
        let device = self
            .start_device_authorization(&oauth_host, cancel.as_ref())
            .await?;
        // `:277-283` — note the COMPLETE URI is what the user is shown.
        interaction.notify(AuthEvent::DeviceCode {
            user_code: device.user_code.clone(),
            verification_uri: device.verification_uri_complete.clone(),
            interval_seconds: Some(device.interval_seconds),
            expires_in_seconds: Some(device.expires_in_seconds),
        });
        let token = self.poll_for_token(&oauth_host, &device, cancel).await?;
        // `:285`
        Ok(token.into_credential())
    }
}

/// The `poll` callback of `pollForToken` (`kimi-coding.ts:151-205`).
struct TokenPoller<'a> {
    flow: &'a KimiCodingOAuth,
    url: String,
    device_code: String,
    cancel: Option<CancelToken>,
}

#[async_trait::async_trait]
impl DeviceCodePoller for TokenPoller<'_> {
    type Value = TokenResponse;

    async fn poll(&self) -> Result<DeviceCodePollResult<TokenResponse>, OAuthError> {
        // `:158-162`
        let body = encode_query([
            ("client_id", CLIENT_ID),
            ("device_code", self.device_code.as_str()),
            ("grant_type", DEVICE_CODE_GRANT_TYPE),
        ]);
        // A transport failure rejects the whole flow, as upstream's unguarded `await fetch` does.
        let reply = self
            .flow
            .send(&self.url, body, self.cancel.as_ref())
            .await?;
        Ok(classify_device_token_reply(
            reply.status,
            reply.json().as_ref(),
            &reply.text,
            super::now_ms(),
        ))
    }
}

#[async_trait::async_trait]
impl OAuthAuth for KimiCodingOAuth {
    /// `kimi-coding.ts:289`.
    fn name(&self) -> &str {
        "Kimi Code (subscription)"
    }

    /// `isSubscription: true` (pi v0.84.1 `oauth/kimi-coding.ts:297`) — a Kimi Code plan.
    fn is_subscription(&self) -> bool {
        true
    }

    /// `kimi-coding.ts:290`.
    fn login_label(&self) -> Option<&str> {
        Some("Sign in with Kimi Code")
    }

    /// 1:1 port of `loginKimiCoding` (`kimi-coding.ts:274-286`, wired at `:292`): start the device
    /// authorization, show the user code and the complete verification URI, then poll the token
    /// endpoint until the user approves.
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        self.run_login(interaction).await
    }

    /// `kimi-coding.ts:294-297`. The upstream signature also takes an `AbortSignal`, which this
    /// trait has no parameter for; [`KimiCodingOAuth::refresh_token`] is the cancellable form.
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        let refresh = match cred {
            Credential::Oauth { refresh, .. } => refresh.clone(),
            Credential::ApiKey { .. } => {
                return Err(OAuthError::Failed(
                    "Kimi Code OAuth refresh requires an oauth credential".to_string(),
                )
                .into_auth_error(KIMI_CODING_PROVIDER_ID));
            }
        };
        let host = self.oauth_host().await;
        self.refresh_token(&host, &refresh, None)
            .await
            .map(TokenResponse::into_credential)
            .map_err(|e| e.into_auth_error(KIMI_CODING_PROVIDER_ID))
    }

    /// `kimi-coding.ts:299-301` — `{ headers: { Authorization: "Bearer <access>" } }`. Note this
    /// is a HEADER, not `apiKey`: Kimi Code's coding endpoint is not keyed off the standard
    /// provider key slot.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        let access = match cred {
            Credential::Oauth { access, .. } => access.clone(),
            Credential::ApiKey { .. } => {
                return Err(OAuthError::Failed(
                    "Kimi Code OAuth toAuth requires an oauth credential".to_string(),
                )
                .into_auth_error(KIMI_CODING_PROVIDER_ID));
            }
        };
        let mut headers = crate::HeaderMap::new();
        headers.insert(
            "Authorization".to_string(),
            Some(format!("Bearer {access}")),
        );
        Ok(ModelAuth {
            api_key: None,
            headers: Some(headers),
            base_url: None,
        })
    }
}

/// An [`super::load::OAuthFlowFactory`]-shaped constructor for
/// [`super::load::register_bundled_oauth_flow_loaders`] under
/// [`super::load::OAuthFlowId::KimiCoding`] (`load.ts:51-54`).
pub fn kimi_coding_oauth_flow() -> Result<Arc<dyn OAuthAuth>, OAuthError> {
    Ok(Arc::new(KimiCodingOAuth::new()) as Arc<dyn OAuthAuth>)
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
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // ------------------------------------------------------------------ upstream fixtures

    /// The RFC 8628 device-authorization payload Kimi returns (`kimi-coding.ts:86-89`).
    fn device_body() -> Value {
        json!({
            "device_code": "GmRhmhcxhwAzkoEqiMEg_DnyEysNkuNhszIySk9eS",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://auth.kimi.com/device",
            "verification_uri_complete": "https://auth.kimi.com/device?user_code=WDJB-MJHT",
            "interval": 1,
            "expires_in": 900,
        })
    }

    // ------------------------------------------------------------------ loopback harness

    #[derive(Clone, Debug)]
    struct Recorded {
        line: String,
        head: String,
        body: String,
    }

    type Router = Arc<dyn Fn(&str, usize) -> (u16, String) + Send + Sync>;

    /// A loopback HTTP server; `router(path, nth_hit_for_that_path)` picks the reply so a poll or
    /// retry sequence is scriptable. Nothing in this file can reach a real host.
    async fn spawn(router: Router) -> (String, Arc<Mutex<Vec<Recorded>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let log: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&log);
        tokio::spawn(async move {
            let hits: Arc<Mutex<BTreeMap<String, usize>>> = Arc::new(Mutex::new(BTreeMap::new()));
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let sink = Arc::clone(&sink);
                let router = Arc::clone(&router);
                let hits = Arc::clone(&hits);
                tokio::spawn(async move {
                    let mut raw: Vec<u8> = Vec::new();
                    let mut buf = [0u8; 1024];
                    let head_end = loop {
                        match sock.read(&mut buf).await {
                            Ok(0) => break raw.len(),
                            Ok(n) => {
                                raw.extend_from_slice(&buf[..n]);
                                if let Some(idx) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                                    break idx + 4;
                                }
                            }
                            Err(_) => return,
                        }
                    };
                    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
                    let content_length = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    while raw.len() < head_end + content_length {
                        match sock.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => raw.extend_from_slice(&buf[..n]),
                            Err(_) => return,
                        }
                    }
                    let body = String::from_utf8_lossy(&raw[head_end..]).to_string();
                    let line = head.lines().next().unwrap_or_default().to_string();
                    let path = line.split(' ').nth(1).unwrap_or("/").to_string();

                    let nth = {
                        let mut hits = hits.lock().unwrap();
                        let counter = hits.entry(path.clone()).or_insert(0);
                        let nth = *counter;
                        *counter += 1;
                        nth
                    };
                    sink.lock().unwrap().push(Recorded { line, head, body });

                    let (status, reply) = router(&path, nth);
                    let response = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        (format!("http://{addr}"), log)
    }

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
    fn env_with(pairs: &[(&str, &str)]) -> Arc<dyn AuthContext> {
        Arc::new(MapEnv(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        ))
    }

    /// A flow whose OAuth host is the loopback listener — set through the SAME env override
    /// upstream honours, so this exercises `getOauthHost` rather than a test-only backdoor.
    fn flow_at(origin: &str) -> KimiCodingOAuth {
        KimiCodingOAuth::with_auth_context(env_with(&[(OAUTH_HOST_ENV, origin)]))
    }

    // ------------------------------------------------------------------ getOauthHost

    /// `kimi-coding.ts:35-38`: the two overrides in precedence order, the default, and the
    /// trailing-slash strip.
    #[tokio::test]
    async fn oauth_host_resolution_matches_upstream() {
        assert_eq!(
            KimiCodingOAuth::with_auth_context(env_with(&[]))
                .oauth_host()
                .await,
            "https://auth.kimi.com"
        );
        assert_eq!(
            KimiCodingOAuth::with_auth_context(env_with(&[(
                OAUTH_HOST_ENV_LEGACY,
                "https://legacy.example"
            )]))
            .oauth_host()
            .await,
            "https://legacy.example"
        );
        // `KIMI_CODE_OAUTH_HOST || KIMI_OAUTH_HOST` — the first wins.
        assert_eq!(
            KimiCodingOAuth::with_auth_context(env_with(&[
                (OAUTH_HOST_ENV, "https://scoped.example"),
                (OAUTH_HOST_ENV_LEGACY, "https://legacy.example"),
            ]))
            .oauth_host()
            .await,
            "https://scoped.example"
        );
        // An EMPTY scoped value is falsy in JS and falls through to the legacy one.
        assert_eq!(
            KimiCodingOAuth::with_auth_context(env_with(&[
                (OAUTH_HOST_ENV, ""),
                (OAUTH_HOST_ENV_LEGACY, "https://legacy.example"),
            ]))
            .oauth_host()
            .await,
            "https://legacy.example"
        );
        // `.replace(/\/+$/, "")` strips every trailing slash.
        assert_eq!(
            KimiCodingOAuth::with_auth_context(env_with(&[(
                OAUTH_HOST_ENV,
                "https://h.example///"
            )]))
            .oauth_host()
            .await,
            "https://h.example"
        );
    }

    // ------------------------------------------------------------------ trustedHttpUrl

    /// `kimi-coding.ts:57-67`. The MIRROR case (the URI Kimi actually returns) stays green so the
    /// rejections are not vacuous.
    #[test]
    fn trusted_http_url_admits_only_http_and_https() {
        assert_eq!(
            trusted_http_url("https://auth.kimi.com/device?user_code=WDJB-MJHT").as_deref(),
            Some("https://auth.kimi.com/device?user_code=WDJB-MJHT")
        );
        assert_eq!(
            trusted_http_url("http://127.0.0.1:9000/device").as_deref(),
            Some("http://127.0.0.1:9000/device")
        );
        assert_eq!(trusted_http_url(""), None);
        assert_eq!(trusted_http_url("file:///etc/passwd"), None);
        assert_eq!(trusted_http_url("javascript:alert(1)"), None);
        assert_eq!(trusted_http_url("data:text/html,<script>"), None);
        assert_eq!(trusted_http_url("auth.kimi.com/device"), None);
        assert_eq!(trusted_http_url("https://"), None);
        // `.href` lowercases the scheme and host and gives an empty path a `/`.
        assert_eq!(
            trusted_http_url("HTTPS://Auth.Kimi.COM").as_deref(),
            Some("https://auth.kimi.com/")
        );
    }

    // ------------------------------------------------------------------ device authorization

    /// The happy path and the two defaults (`kimi-coding.ts:103-116`).
    #[test]
    fn device_authorization_parses_and_defaults() {
        let device = parse_device_authorization(Some(&device_body())).unwrap();
        assert_eq!(device.user_code, "WDJB-MJHT");
        assert_eq!(
            device.verification_uri_complete,
            "https://auth.kimi.com/device?user_code=WDJB-MJHT"
        );
        assert_eq!(device.interval_seconds, 1.0);
        assert_eq!(device.expires_in_seconds, 900.0);

        // Absent / non-positive hints fall back to 5 s and 15 min (`:108-115`).
        let mut bare = device_body();
        let obj = bare.as_object_mut().unwrap();
        obj.remove("interval");
        obj.remove("expires_in");
        let device = parse_device_authorization(Some(&bare)).unwrap();
        assert_eq!(device.interval_seconds, 5.0);
        assert_eq!(device.expires_in_seconds, 900.0);

        let mut zeroed = device_body();
        let obj = zeroed.as_object_mut().unwrap();
        obj["interval"] = json!(0);
        obj["expires_in"] = json!(-1);
        let device = parse_device_authorization(Some(&zeroed)).unwrap();
        assert_eq!(device.interval_seconds, 5.0);
        assert_eq!(device.expires_in_seconds, 900.0);
    }

    /// `kimi-coding.ts:90-99`: a missing field or an untrusted URI is the same message, which
    /// embeds the raw body.
    #[test]
    fn device_authorization_guards() {
        let err = |v: Option<&Value>| parse_device_authorization(v).unwrap_err().to_string();

        assert_eq!(
            err(None),
            "Invalid Kimi Code device authorization response: null"
        );
        assert_eq!(
            err(Some(&json!({"error": "nope"}))),
            r#"Invalid Kimi Code device authorization response: {"error":"nope"}"#
        );

        let mut hostile = device_body();
        hostile.as_object_mut().unwrap()["verification_uri_complete"] =
            json!("javascript:alert(1)");
        assert!(
            err(Some(&hostile)).starts_with("Invalid Kimi Code device authorization response: {"),
            "an untrusted complete URI is rejected by the same guard"
        );

        let mut missing = device_body();
        missing.as_object_mut().unwrap().remove("device_code");
        assert!(
            err(Some(&missing)).starts_with("Invalid Kimi Code device authorization response:")
        );
    }

    /// The request shape (`kimi-coding.ts:70-77`): POST, form-encoded `client_id` only.
    #[tokio::test]
    async fn start_device_authorization_sends_the_upstream_request() {
        let body = device_body().to_string();
        let (origin, log) = spawn(Arc::new(move |_, _| (200, body.clone()))).await;
        let flow = flow_at(&origin);
        let host = flow.oauth_host().await;
        let device = flow.start_device_authorization(&host, None).await.unwrap();
        assert_eq!(device.user_code, "WDJB-MJHT");

        let recorded = log.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].line.split(' ').next().unwrap(),
            "POST",
            "RFC 8628 §3.1 is a POST"
        );
        assert_eq!(
            recorded[0].line.split(' ').nth(1).unwrap(),
            "/api/oauth/device_authorization"
        );
        assert_eq!(
            recorded[0].body,
            "client_id=17e5f671-d194-4dfb-9706-5516cb48c098"
        );
        let head = recorded[0].head.to_ascii_lowercase();
        assert!(
            head.contains("content-type: application/x-www-form-urlencoded"),
            "{head}"
        );
        assert!(head.contains("accept: application/json"), "{head}");
    }

    /// `kimi-coding.ts:80-83`.
    #[tokio::test]
    async fn non_2xx_device_authorization_reports_status_and_body() {
        let (origin, _) = spawn(Arc::new(|_, _| (502, "bad gateway".to_string()))).await;
        let flow = flow_at(&origin);
        let host = flow.oauth_host().await;
        let err = flow
            .start_device_authorization(&host, None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Kimi Code device authorization failed with status 502: bad gateway"
        );
    }

    // ------------------------------------------------------------------ token parsing

    /// `parseTokenResponse` (`kimi-coding.ts:119-139`) — including the exact deadline arithmetic,
    /// which carries NO safety skew unlike the Anthropic and Copilot flows.
    #[test]
    fn token_response_parsing_and_expiry() {
        let json =
            json!({"access_token": "kc_access", "refresh_token": "kc_refresh", "expires_in": 3600});
        let token = parse_token_response(Some(&json), "poll", 1_700_000_000_000).unwrap();
        assert_eq!(
            token,
            TokenResponse {
                access: "kc_access".into(),
                refresh: "kc_refresh".into(),
                expires: 1_700_000_000_000 + 3_600_000,
            }
        );

        // Every field is required, non-empty and positive; `operation` is spliced in verbatim.
        let err = |v: Value, op: &str| {
            parse_token_response(Some(&v), op, 0)
                .unwrap_err()
                .to_string()
        };
        // ACP-Q1 — the echoed payload keeps the response's own key order (`serde_json/preserve_order`,
        // turned on graph-wide by `agent-client-protocol`, `cyrup-acp`'s wire dependency), so this
        // user-visible string is now byte-identical to upstream's `JSON.stringify`. It used to be
        // alphabetised by `BTreeMap`; see `openai_codex`'s
        // `the_echoed_payload_matches_json_stringify_insertion_order` for the same change stated in
        // full.
        assert_eq!(
            err(json!({"refresh_token": "r", "expires_in": 1}), "refresh"),
            r#"Kimi Code token refresh response missing fields: {"refresh_token":"r","expires_in":1}"#
        );
        assert!(
            err(
                json!({"access_token": "", "refresh_token": "r", "expires_in": 1}),
                "poll"
            )
            .starts_with("Kimi Code token poll response missing fields:")
        );
        assert!(
            err(
                json!({"access_token": "a", "refresh_token": "r", "expires_in": 0}),
                "poll"
            )
            .starts_with("Kimi Code token poll response missing fields:")
        );
        assert_eq!(
            parse_token_response(None, "poll", 0)
                .unwrap_err()
                .to_string(),
            "Kimi Code token poll response missing fields: null"
        );
    }

    /// Every branch of `kimi-coding.ts:166-204`, against the literal payloads an RFC 8628 token
    /// endpoint returns.
    #[test]
    fn device_token_reply_classification() {
        let ok = json!({"access_token": "a", "refresh_token": "r", "expires_in": 60});
        assert_eq!(
            classify_device_token_reply(200, Some(&ok), "", 1_000),
            DeviceCodePollResult::Complete(TokenResponse {
                access: "a".into(),
                refresh: "r".into(),
                expires: 1_000 + 60_000,
            })
        );
        // A 5xx short-circuits before any JSON is consulted (`:166-172`).
        assert_eq!(
            classify_device_token_reply(503, None, "gateway down", 0),
            DeviceCodePollResult::Failed {
                message: "Kimi Code device token request failed with status 503: gateway down"
                    .into()
            }
        );
        assert_eq!(
            classify_device_token_reply(500, None, "", 0),
            DeviceCodePollResult::Failed {
                message: "Kimi Code device token request failed with status 500".into()
            }
        );
        assert_eq!(
            classify_device_token_reply(
                400,
                Some(&json!({"error": "authorization_pending"})),
                "",
                0
            ),
            DeviceCodePollResult::Pending
        );
        assert_eq!(
            classify_device_token_reply(
                400,
                Some(&json!({"error": "slow_down", "interval": 9})),
                "",
                0
            ),
            DeviceCodePollResult::SlowDown {
                interval_seconds: Some(9.0)
            }
        );
        assert_eq!(
            classify_device_token_reply(
                400,
                Some(&json!({"error": "slow_down", "interval": 0})),
                "",
                0
            ),
            DeviceCodePollResult::SlowDown {
                interval_seconds: None
            }
        );
        assert_eq!(
            classify_device_token_reply(400, Some(&json!({"error": "expired_token"})), "", 0),
            DeviceCodePollResult::Failed {
                message: "Kimi Code device authorization expired. Please restart login.".into()
            }
        );
        assert_eq!(
            classify_device_token_reply(403, Some(&json!({"error": "access_denied"})), "", 0),
            DeviceCodePollResult::Failed {
                message: "Kimi Code login was denied.".into()
            }
        );
        // An unknown error keeps its description; a bodiless failure has no suffix at all.
        assert_eq!(
            classify_device_token_reply(
                400,
                Some(&json!({"error": "invalid_request", "error_description": "bad device_code"})),
                "",
                0
            ),
            DeviceCodePollResult::Failed {
                message: "Kimi Code device token request failed (status 400): invalid_request: bad device_code".into()
            }
        );
        assert_eq!(
            classify_device_token_reply(418, None, "", 0),
            DeviceCodePollResult::Failed {
                message: "Kimi Code device token request failed (status 418)".into()
            }
        );
        // A 200 whose body is not a completed token still falls through to the error branches.
        assert_eq!(
            classify_device_token_reply(
                200,
                Some(&json!({"error": "authorization_pending"})),
                "",
                0
            ),
            DeviceCodePollResult::Pending
        );
    }

    /// The poll request body is the RFC 8628 device-code grant (`kimi-coding.ts:158-162`).
    #[tokio::test]
    async fn token_poll_sends_the_device_code_grant() {
        let reply =
            json!({"access_token": "a", "refresh_token": "r", "expires_in": 60}).to_string();
        let (origin, log) = spawn(Arc::new(move |_, _| (200, reply.clone()))).await;
        let flow = flow_at(&origin);
        let host = flow.oauth_host().await;
        let poller = TokenPoller {
            flow: &flow,
            url: format!("{host}{TOKEN_PATH}"),
            device_code: "dev-1".to_string(),
            cancel: None,
        };
        assert!(matches!(
            poller.poll().await.unwrap(),
            DeviceCodePollResult::Complete(_)
        ));
        let recorded = log.lock().unwrap().clone();
        assert_eq!(
            recorded[0].line.split(' ').nth(1).unwrap(),
            "/api/oauth/token"
        );
        assert_eq!(
            recorded[0].body,
            "client_id=17e5f671-d194-4dfb-9706-5516cb48c098&device_code=dev-1&grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
        );
    }

    // ------------------------------------------------------------------ refresh

    /// `kimi-coding.ts:239-243` — the RFC 6749 §6 refresh grant, and its success shape.
    #[tokio::test]
    async fn refresh_sends_the_refresh_token_grant() {
        let reply =
            json!({"access_token": "a2", "refresh_token": "r2", "expires_in": 120}).to_string();
        let (origin, log) = spawn(Arc::new(move |_, _| (200, reply.clone()))).await;
        let flow = flow_at(&origin);
        let host = flow.oauth_host().await;
        let token = flow.refresh_token(&host, "r1", None).await.unwrap();
        assert_eq!(token.access, "a2");
        assert_eq!(token.refresh, "r2");
        assert!(token.expires > super::super::now_ms());

        let recorded = log.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1, "a 2xx is not retried");
        assert_eq!(
            recorded[0].body,
            "client_id=17e5f671-d194-4dfb-9706-5516cb48c098&grant_type=refresh_token&refresh_token=r1"
        );
    }

    /// `kimi-coding.ts:256-260`: 401/403/`invalid_grant` are terminal — the credential is dead, so
    /// retrying would only burn time before the re-login prompt.
    #[tokio::test]
    async fn unauthorized_refresh_is_terminal_and_not_retried() {
        let body = json!({"error_description": "token revoked"}).to_string();
        let (origin, log) = spawn(Arc::new(move |_, _| (401, body.clone()))).await;
        let flow = flow_at(&origin);
        let host = flow.oauth_host().await;
        let err = flow.refresh_token(&host, "r1", None).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Kimi Code token refresh unauthorized (status 401): token revoked"
        );
        assert_eq!(log.lock().unwrap().len(), 1, "no retry on 401");

        // `invalid_grant` is terminal even on a status that would otherwise be retryable.
        let body = json!({"error": "invalid_grant"}).to_string();
        let (origin, log) = spawn(Arc::new(move |_, _| (429, body.clone()))).await;
        let flow = flow_at(&origin);
        let host = flow.oauth_host().await;
        let err = flow.refresh_token(&host, "r1", None).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Kimi Code token refresh unauthorized (status 429)"
        );
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    /// `kimi-coding.ts:262-268`: a 400 is NOT retryable, and its message embeds the parsed body.
    #[tokio::test]
    async fn non_retryable_refresh_failure_reports_the_body() {
        let body = json!({"error": "invalid_request"}).to_string();
        let (origin, log) = spawn(Arc::new(move |_, _| (400, body.clone()))).await;
        let flow = flow_at(&origin);
        let host = flow.oauth_host().await;
        let err = flow.refresh_token(&host, "r1", None).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            r#"Kimi Code token refresh failed with status 400: {"error":"invalid_request"}"#
        );
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    /// `kimi-coding.ts:262-265`: a 5xx is retried after a 1 s backoff and the retry's success is
    /// returned. This is the only test that waits out a real backoff.
    #[tokio::test]
    async fn retryable_refresh_failure_is_retried_then_succeeds() {
        let good =
            json!({"access_token": "a2", "refresh_token": "r2", "expires_in": 60}).to_string();
        let (origin, log) = spawn(Arc::new(move |_, nth| {
            if nth == 0 {
                (503, "down".to_string())
            } else {
                (200, good.clone())
            }
        }))
        .await;
        let flow = flow_at(&origin);
        let host = flow.oauth_host().await;
        let started = std::time::Instant::now();
        let token = flow.refresh_token(&host, "r1", None).await.unwrap();
        assert_eq!(token.access, "a2");
        assert_eq!(log.lock().unwrap().len(), 2);
        assert!(
            started.elapsed() >= Duration::from_millis(900),
            "the 1 s backoff at `:225` was skipped: {:?}",
            started.elapsed()
        );
    }

    /// `kimi-coding.ts:227-229`: an already-aborted refresh reports its own message, distinct from
    /// the login-level cancel.
    #[tokio::test]
    async fn aborted_refresh_reports_its_own_message() {
        let (origin, log) = spawn(Arc::new(|_, _| (200, "{}".to_string()))).await;
        let flow = flow_at(&origin);
        let host = flow.oauth_host().await;
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = flow
            .refresh_token(&host, "r1", Some(&cancel))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Kimi Code token refresh aborted");
        assert!(log.lock().unwrap().is_empty());
    }

    // ------------------------------------------------------------------ login + strategy

    /// The full `loginKimiCoding` sequence (`kimi-coding.ts:274-286`) over loopback, driven
    /// through the real [`OAuthAuth::login`] entry point.
    #[tokio::test]
    async fn login_completes_the_device_flow() {
        let device = device_body().to_string();
        let token =
            json!({"access_token": "kc_access", "refresh_token": "kc_refresh", "expires_in": 3600})
                .to_string();
        let (origin, _) = spawn(Arc::new(move |path, _| match path {
            "/api/oauth/device_authorization" => (200, device.clone()),
            "/api/oauth/token" => (200, token.clone()),
            _ => (404, "{}".to_string()),
        }))
        .await;

        let interaction = ScriptedInteraction::new(Vec::new());
        let before = super::super::now_ms();
        let credential = flow_at(&origin).login(&interaction).await.unwrap();

        match &credential {
            Credential::Oauth {
                access,
                refresh,
                expires,
                ext,
            } => {
                assert_eq!(access, "kc_access");
                assert_eq!(refresh, "kc_refresh");
                // `Date.now() + expires_in * 1000`, no skew (`:137`).
                assert!(*expires >= before + 3_600_000);
                assert!(*expires <= super::super::now_ms() + 3_600_000);
                assert!(ext.is_empty(), "Kimi records no extra credential fields");
            }
            Credential::ApiKey { .. } => panic!("login must yield an OAuth credential"),
        }

        // `:277-283` — the COMPLETE URI is what the user is shown, not the bare one.
        assert_eq!(
            interaction.events(),
            vec![AuthEvent::DeviceCode {
                user_code: "WDJB-MJHT".to_string(),
                verification_uri: "https://auth.kimi.com/device?user_code=WDJB-MJHT".to_string(),
                interval_seconds: Some(1.0),
                expires_in_seconds: Some(900.0),
            }]
        );
        // Kimi's login asks the user nothing (unlike GitHub Copilot's enterprise prompt).
        assert!(interaction.prompts().is_empty());
    }

    /// An aborted login never reaches the device endpoint and reports upstream's `CANCEL_MESSAGE`.
    #[tokio::test]
    async fn cancelled_login_reports_login_cancelled() {
        let (origin, log) = spawn(Arc::new(|_, _| (200, "{}".to_string()))).await;
        let cancel = CancelToken::new();
        cancel.cancel();
        let interaction = ScriptedInteraction::new(Vec::new()).with_cancel(cancel);
        let err = flow_at(&origin).login(&interaction).await.unwrap_err();
        assert_eq!(err.to_string(), "Login cancelled");
        assert!(log.lock().unwrap().is_empty());
    }

    /// The strategy object (`kimi-coding.ts:288-302`).
    #[tokio::test]
    async fn strategy_name_label_and_to_auth() {
        let flow = KimiCodingOAuth::new();
        assert_eq!(flow.name(), "Kimi Code (subscription)");
        assert_eq!(flow.login_label(), Some("Sign in with Kimi Code"));

        let cred = super::super::oauth_credential("kc_access", "kc_refresh", 0);
        let auth = flow.to_auth(&cred).await.unwrap();
        assert_eq!(
            auth.headers.as_ref().and_then(|h| h.get("Authorization")),
            Some(&Some("Bearer kc_access".to_string())),
            "`:299-301` sends a header, not an apiKey"
        );
        assert!(auth.api_key.is_none());
        assert!(auth.base_url.is_none());
    }

    /// The factory is registry-shaped (`load.ts:51-54`).
    #[test]
    fn factory_builds_the_flow() {
        assert_eq!(
            kimi_coding_oauth_flow().unwrap().name(),
            "Kimi Code (subscription)"
        );
    }
}
