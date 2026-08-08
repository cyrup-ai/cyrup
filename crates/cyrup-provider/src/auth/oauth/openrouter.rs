//! OpenRouter OAuth PKCE flow — 1:1 port of pi v0.83.0
//! `packages/ai/src/auth/oauth/openrouter.ts` (311 lines).
//!
//! Upstream's own header states the shape: "OpenRouter exchanges an authorization code for a
//! permanent, user-controlled API key rather than an expiring access/refresh token pair. The
//! callback is handled by a one-shot loopback server on an ephemeral port, raced against a manual
//! prompt so remote/headless sessions can paste the redirect URL when the browser cannot reach the
//! loopback server." (`openrouter.ts:1-12`).
//!
//! ## Provenance
//!
//! | this module | `openrouter.ts` |
//! |---|---|
//! | [`AUTHORIZE_URL`] / [`TOKEN_URL`] / [`LOGIN_TIMEOUT`] / [`TOKEN_EXCHANGE_TIMEOUT`] | `:20-23` |
//! | [`OpenRouterOAuth::bind_host`] | `getCallbackHost`, `:25-27` |
//! | [`parse_authorization_input`] | `parseAuthorizationInput`, `:52-67` |
//! | [`error_detail`] | `errorDetail`, `:69-78` |
//! | [`OpenRouterOAuth::exchange_authorization_code`] | `exchangeAuthorizationCode`, `:80-133` |
//! | [`OpenRouterCallbackHandler`] | the `createServer` request handler, `:169-206` |
//! | [`OpenRouterOAuth::run_login`] | `loginOpenRouter`, `:242-299` |
//! | `impl OAuthAuth for OpenRouterOAuth` | `openRouterOAuth`, `:301-311` |
//!
//! The listener itself (`startCallbackServer`, `:135-240`) is [`super::callback::CallbackServer`]:
//! that module was factored out of this very function, so `close`/`cancelWait`/`waitForCredential`
//! and the claim/settle latches live there rather than here. PKCE, the callback pages, the v4 UUID
//! and `URLSearchParams` likewise come from the shared [`super`] substrate.
//!
//! ## Mechanism divergences (Rust forces these; behaviour is unchanged)
//!
//! * **Callback server.** Upstream calls `node:http.createServer` (`:14`, `:169`). This crate's
//!   `tokio` carries no `net` feature, so [`super::callback::CallbackServer`] runs a
//!   `std::net::TcpListener` accept loop on a dedicated thread. Two consequences worth naming,
//!   because they are the only places the wire bytes differ from upstream's:
//!   1. The shared server checks the *route* before the claimed/settled latch, and does not check
//!      the method at all, whereas `:172` tests `method !== "GET" || pathname !== callbackPath`
//!      together. The method half is therefore re-checked in [`OpenRouterCallbackHandler`], which
//!      puts it *after* the 409 latch: a non-GET request to the callback path arriving after the
//!      login is already claimed answers 409 where upstream answers 404. A browser following an
//!      OAuth redirect only ever issues GET, so no real client can observe this.
//!   2. `sendHtml` (`:45-50`) sets `cache-control: no-store` on **every** reply. Each reply this
//!      module builds sets it; the shared server's own 400/404/409 pages do not.
//! * **No ambient `fetch`.** The token exchange is `reqwest`. `AbortSignal` is a
//!   [`CancelToken`] (arch-00 §3.2), and `AbortSignal.timeout`-style abort at `:89-92` is
//!   `reqwest`'s per-request `.timeout()`, whose expiry is reported as `is_timeout()` and mapped
//!   back to upstream's `"OpenRouter OAuth token exchange timed out"` message.
//! * **`new URL(...)`.** Rust has no URL parser in this crate's dependency graph, so
//!   [`absolute_url_query`] approximates the WHATWG scheme-start state — the whole discrimination
//!   `:56-60` needs, since an authorization code carries no scheme.
//! * **Error type.** Upstream `throw new Error(msg)`; here the message is the `Display` of an
//!   [`OAuthError`]. Every message string is preserved verbatim.
//! * **Endpoint override.** Upstream's tests stub the ambient `fetch`; Rust has none, so the
//!   endpoints are struct fields ([`OpenRouterOAuth::with_endpoints`]) defaulting to the upstream
//!   constants. Production callers use [`OpenRouterOAuth::new`].
//! * `:225-229`'s `"Could not determine the OpenRouter OAuth callback port"` has no Rust
//!   counterpart: `TcpListener::local_addr` always yields a `SocketAddr`, never a string, so the
//!   branch is unreachable rather than omitted.

use super::callback::{
    CallbackControl, CallbackHandler, CallbackOutcome, CallbackReply, CallbackRequest,
    CallbackServer, CallbackServerConfig, callback_host,
};
use super::interaction::{AuthEvent, AuthInteraction, AuthPrompt};
use super::pkce::generate_pkce;
use super::query::{encode_query, parse_query};
use super::random::random_uuid_v4;
use super::{OAuthError, oauth_credential};
use crate::auth::OAuthAuth;
use crate::auth::types::{Credential, EnvAuthContext, ModelAuth};
use crate::error::AuthError;
use cyrup_core::CancelToken;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants — openrouter.ts:20-23
// ---------------------------------------------------------------------------

/// `openrouter.ts:20`.
pub const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
/// `openrouter.ts:21`.
pub const TOKEN_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
/// `LOGIN_TIMEOUT_MS = 5 * 60 * 1000` (`openrouter.ts:22`).
pub const LOGIN_TIMEOUT: Duration = Duration::from_millis(5 * 60 * 1000);
/// `TOKEN_EXCHANGE_TIMEOUT_MS = 30_000` (`openrouter.ts:23`).
pub const TOKEN_EXCHANGE_TIMEOUT: Duration = Duration::from_millis(30_000);

/// `openrouter.ts:223`.
const LOGIN_TIMEOUT_MESSAGE: &str = "OpenRouter OAuth login timed out";
/// `openrouter.ts:90`, `:111`.
const EXCHANGE_TIMEOUT_MESSAGE: &str = "OpenRouter OAuth token exchange timed out";
/// `openrouter.ts:107`.
const INVALID_JSON_MESSAGE: &str = "OpenRouter OAuth returned invalid JSON";
/// `openrouter.ts:124`.
const NO_KEY_MESSAGE: &str = r#"OpenRouter OAuth response carries no "key""#;
/// `openrouter.ts:292`.
const MISSING_CODE_MESSAGE: &str = "Missing authorization code";
/// `openrouter.ts:265-266`.
const AUTH_URL_INSTRUCTIONS: &str = "Complete sign-in in your browser. If the browser is on another machine, paste the final redirect URL here.";
/// `openrouter.ts:272`.
const MANUAL_PROMPT_MESSAGE: &str =
    "Complete sign-in in your browser, or paste the authorization code / redirect URL here:";
/// `openrouter.ts:293`.
const EXCHANGE_PROGRESS_MESSAGE: &str = "Exchanging authorization code for an API key...";

/// `Number.MAX_SAFE_INTEGER` (`openrouter.ts:131`) — the OpenRouter key never expires, and this is
/// the sentinel upstream writes into `auth.json` to say so.
pub const NEVER_EXPIRES: i64 = 9_007_199_254_740_991;

/// JS truthiness for an optional string: `undefined`/`null` and `""` are both falsy. Upstream
/// depends on it at `:182`, `:190`, `:287`, `:292` and in the `detail ? ... : ""` template at
/// `:120` — `searchParams.get()` yields `""` for a bare `?code=`, which has to read as missing.
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// parseAuthorizationInput — openrouter.ts:52-67
// ---------------------------------------------------------------------------

/// The query component of an absolute URL — what `new URL(value).searchParams` reads — or `None`
/// when `new URL(value)` would throw.
///
/// Approximates the WHATWG parser's scheme-start state: ASCII alpha, then
/// alpha/digit/`+`/`-`/`.`, then `:`. A fragment is not part of `searchParams`, so a `#` before
/// any `?` means there is no query.
fn absolute_url_query(value: &str) -> Option<&str> {
    let (scheme, rest) = value.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') {
        return None;
    }
    let before_fragment = rest.split('#').next().unwrap_or("");
    Some(before_fragment.split_once('?').map_or("", |(_, q)| q))
}

/// `searchParams.get(name)` over a raw query string — the first occurrence wins.
fn query_get(query: &str, key: &str) -> Option<String> {
    parse_query(query)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

/// 1:1 port of `parseAuthorizationInput` (`openrouter.ts:52-67`). Accepts, in order: a full
/// redirect URL, a raw `code=…` query string, or a bare code.
///
/// Note this is the *shorter* sibling of `anthropic.ts:52-79`: OpenRouter carries no `state`, and
/// there is no `code#state` branch. `None` is upstream's `undefined`; `Some("")` is the empty
/// string `searchParams.get` yields for a bare `?code=`, which the caller's `if (!code)` (`:292`)
/// still treats as missing — see [`truthy`].
pub fn parse_authorization_input(input: &str) -> Option<String> {
    // `:53-54`
    let value = input.trim();
    if value.is_empty() {
        return None;
    }

    // `:56-60` — a parseable URL wins outright, even when it carries no `code`; the `catch` is
    // only entered when `new URL` *throws*, never when the lookup misses.
    if let Some(query) = absolute_url_query(value) {
        return query_get(query, "code");
    }

    // `:62-64`
    if value.contains("code=") {
        return query_get(value, "code");
    }

    // `:66`
    Some(value.to_string())
}

// ---------------------------------------------------------------------------
// errorDetail — openrouter.ts:69-78
// ---------------------------------------------------------------------------

/// 1:1 port of `errorDetail` (`openrouter.ts:69-78`): `error_description`, then `message`, then a
/// string `error`, then `error.message` when `error` is a non-array object.
pub fn error_detail(body: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    if let Some(value) = body.get("error_description").and_then(|v| v.as_str()) {
        return Some(value.to_string());
    }
    if let Some(value) = body.get("message").and_then(|v| v.as_str()) {
        return Some(value.to_string());
    }
    if let Some(value) = body.get("error").and_then(|v| v.as_str()) {
        return Some(value.to_string());
    }
    // `:73-76` — `!Array.isArray(...)` is implicit: a JSON array is not a `Value::Object`.
    if let Some(serde_json::Value::Object(error)) = body.get("error")
        && let Some(message) = error.get("message").and_then(|v| v.as_str())
    {
        return Some(message.to_string());
    }
    None
}

/// `${detail ? `: ${detail}` : ""}` (`openrouter.ts:120`) — JS truthiness, so an empty detail
/// contributes nothing.
fn detail_suffix(detail: Option<&str>) -> String {
    match detail {
        Some(detail) if !detail.is_empty() => format!(": {detail}"),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// The callback handler — openrouter.ts:169-206
// ---------------------------------------------------------------------------

/// The OpenRouter-specific half of the callback server (`openrouter.ts:169-206`).
///
/// Unlike every other ported flow this handler performs the **token exchange itself**, inside the
/// request, so the browser sees either the success page or the exchange failure (`:196-204`). That
/// is why it claims the login first (`:194`): a claimed callback owns the exchange, and
/// [`CallbackServer::cancel_wait`] must no longer be able to hand the login to manual entry.
struct OpenRouterCallbackHandler {
    flow: OpenRouterOAuth,
    /// The PKCE verifier this login started with (`:137`).
    verifier: String,
    /// `interaction.signal`, threaded into the exchange (`:197`).
    cancel: Option<CancelToken>,
}

#[async_trait::async_trait]
impl CallbackHandler for OpenRouterCallbackHandler {
    type Value = Credential;

    async fn handle(
        &self,
        request: CallbackRequest,
        control: CallbackControl,
    ) -> CallbackOutcome<Credential> {
        // `:172-175` — the pathname half is checked by the shared server; this is the method half.
        if request.method != "GET" {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(404, "OAuth callback route not found.", None).no_store(),
            };
        }

        // `:181-187` — JS truthiness, so a bare `?error=` is not an error.
        let oauth_error = request.param("error");
        if truthy(oauth_error) {
            let oauth_error = oauth_error.unwrap_or_default();
            // `:183` — `??`, so a present-but-empty `error_description` is kept.
            let description = request.param("error_description").unwrap_or(oauth_error);
            return CallbackOutcome::Failed {
                reply: CallbackReply::error(
                    400,
                    "OpenRouter authorization was denied.",
                    Some(description),
                )
                .no_store(),
                error: OAuthError::Failed(format!(
                    "OpenRouter authorization failed: {description}"
                )),
            };
        }

        // `:189-193`
        let code = request.param("code");
        if !truthy(code) {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(
                    400,
                    "OpenRouter returned no authorization code.",
                    None,
                )
                .no_store(),
            };
        }
        let code = code.unwrap_or_default().to_string();
        // `:194` — from here on this callback owns the login.
        let _ = control.claim();

        // `:196-204`
        match self
            .flow
            .exchange_authorization_code(&code, &self.verifier, self.cancel.as_ref())
            .await
        {
            Ok(credential) => CallbackOutcome::Complete {
                reply: CallbackReply::success(
                    "Signed in to OpenRouter. You may now close this page.",
                )
                .no_store(),
                value: credential,
            },
            Err(error) => CallbackOutcome::Failed {
                reply: CallbackReply::error(
                    502,
                    "OpenRouter key exchange failed.",
                    Some(&error.to_string()),
                )
                .no_store(),
                error,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// openRouterOAuth — openrouter.ts:301-311
// ---------------------------------------------------------------------------

/// The OpenRouter OAuth strategy — upstream's `openRouterOAuth` (`openrouter.ts:301-311`).
#[derive(Clone, Debug)]
pub struct OpenRouterOAuth {
    authorize_url: String,
    token_url: String,
    /// `None` resolves `CYRUP_OAUTH_CALLBACK_HOST`/`PI_OAUTH_CALLBACK_HOST` at login time, which
    /// is what upstream's `getCallbackHost()` does per login (`openrouter.ts:25-27`, `:141`).
    callback_host: Option<String>,
}

impl Default for OpenRouterOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenRouterOAuth {
    /// The production strategy — upstream's `:20-21` endpoints.
    pub fn new() -> Self {
        OpenRouterOAuth {
            authorize_url: AUTHORIZE_URL.to_string(),
            token_url: TOKEN_URL.to_string(),
            callback_host: None,
        }
    }

    /// Point the flow at different endpoints and a fixed bind host. Exists only because Rust has
    /// no ambient `fetch` for a test to stub the way upstream's would; production code uses
    /// [`OpenRouterOAuth::new`]. The port stays ephemeral, as `:210` binds it.
    pub fn with_endpoints(
        authorize_url: impl Into<String>,
        token_url: impl Into<String>,
        callback_host: impl Into<String>,
    ) -> Self {
        OpenRouterOAuth {
            authorize_url: authorize_url.into(),
            token_url: token_url.into(),
            callback_host: Some(callback_host.into()),
        }
    }

    /// `getCallbackHost()` (`openrouter.ts:25-27`).
    async fn bind_host(&self) -> String {
        match &self.callback_host {
            Some(host) => host.clone(),
            None => callback_host(&EnvAuthContext, None).await,
        }
    }

    /// The browser URL the user must open — `openrouter.ts:251-256`. Parameter order is upstream's
    /// insertion order, which `URLSearchParams.toString()` preserves. Note OpenRouter names the
    /// redirect `callback_url`, not `redirect_uri`, and sends no `client_id`: the ephemeral
    /// callback URL *is* the client identity.
    pub fn authorization_url(&self, callback_url: &str, challenge: &str) -> String {
        let params = encode_query([
            ("callback_url", callback_url),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
        ]);
        format!("{}?{params}", self.authorize_url)
    }

    fn client(&self) -> Result<reqwest::Client, OAuthError> {
        crate::stream::sse::build_client().map_err(|e| OAuthError::Failed(e.to_string()))
    }

    /// 1:1 port of `exchangeAuthorizationCode` (`openrouter.ts:80-133`).
    ///
    /// The response is a *permanent* API key, not a token pair: `refresh` is `""` and `expires` is
    /// [`NEVER_EXPIRES`] (`:127-132`).
    pub async fn exchange_authorization_code(
        &self,
        code: &str,
        verifier: &str,
        cancel: Option<&CancelToken>,
    ) -> Result<Credential, OAuthError> {
        // `:85`
        if cancel.is_some_and(CancelToken::is_cancelled) {
            return Err(OAuthError::Cancelled);
        }
        let client = self.client()?;
        // `:100`
        let payload = serde_json::json!({
            "code": code,
            "code_verifier": verifier,
            "code_challenge_method": "S256",
        });

        // `:97-102` — headers verbatim, and `TOKEN_EXCHANGE_TIMEOUT_MS` as the request deadline.
        let send = client
            .post(&self.token_url)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .timeout(TOKEN_EXCHANGE_TIMEOUT)
            .json(&payload)
            .send();

        let response = match with_cancel(send, cancel).await? {
            Ok(response) => response,
            // `:109-112`
            Err(error) => return Err(map_transport_error(&error, cancel, None)),
        };

        let status = response.status();
        let ok = status.is_success();

        // `:103-108` — the body read and the JSON parse share one `catch`: on a 2xx either failure
        // is `"OpenRouter OAuth returned invalid JSON"`, and on a non-2xx both leave `body = {}`
        // so the `:118-121` branch reports the status alone.
        let text = match with_cancel(response.text(), cancel).await? {
            Ok(text) => Some(text),
            Err(error) => {
                if ok {
                    return Err(map_transport_error(&error, cancel, Some(INVALID_JSON_MESSAGE)));
                }
                None
            }
        };

        let empty = serde_json::Map::new();
        let body = match text.as_deref().map(serde_json::from_str::<serde_json::Value>) {
            // `:105` — only a non-array object is adopted; anything else leaves `body = {}`.
            Some(Ok(serde_json::Value::Object(map))) => map,
            Some(Ok(_)) => empty,
            Some(Err(_)) if ok => return Err(OAuthError::Failed(INVALID_JSON_MESSAGE.to_string())),
            Some(Err(_)) | None => empty,
        };

        // `:118-121`
        if !ok {
            let detail = error_detail(&body);
            return Err(OAuthError::Failed(format!(
                "OpenRouter OAuth key exchange failed (HTTP {}){}",
                status.as_u16(),
                detail_suffix(detail.as_deref())
            )));
        }

        // `:123-125`
        let key = body.get("key").and_then(|v| v.as_str()).unwrap_or_default();
        if key.is_empty() {
            return Err(OAuthError::Failed(NO_KEY_MESSAGE.to_string()));
        }

        // `:127-132`
        Ok(oauth_credential(key, "", NEVER_EXPIRES))
    }

    /// The body of [`OAuthAuth::login`] — `loginOpenRouter` (`openrouter.ts:242-299`) minus the
    /// `finally` block, which [`Self::run_login`] owns.
    async fn login_body(
        &self,
        interaction: &dyn AuthInteraction,
        server: &CallbackServer<Credential>,
        challenge: &str,
        verifier: &str,
        manual_abort: &CancelToken,
    ) -> Result<Credential, OAuthError> {
        let callback_url = server.redirect_uri().to_string();

        // `:258-261`
        interaction.notify(AuthEvent::Progress {
            message: format!("Listening for OpenRouter OAuth callback on {callback_url}"),
        });
        // `:262-267`
        interaction.notify(AuthEvent::AuthUrl {
            url: self.authorization_url(&callback_url, challenge),
            instructions: Some(AUTH_URL_INSTRUCTIONS.to_string()),
        });

        // `:269-283` — the manual prompt runs concurrently with the wait and calls `cancelWait`
        // whichever way it settles. Both futures are pinned rather than `select!`-owned because a
        // dropped `wait()` would take its result channel with it; the paste branch still has to
        // consult the listener afterwards (`:285`).
        let mut manual = Box::pin(
            interaction.prompt(
                AuthPrompt::manual_code(MANUAL_PROMPT_MESSAGE)
                    .with_placeholder(callback_url.clone())
                    .with_cancel(manual_abort.clone()),
            ),
        );
        let mut waiter = Box::pin(server.wait());

        enum Winner {
            Redirect(Result<Option<Credential>, OAuthError>),
            Manual(Result<String, OAuthError>),
        }
        let winner = tokio::select! {
            settled = &mut waiter => Winner::Redirect(settled),
            prompted = &mut manual => Winner::Manual(prompted),
        };

        let mut manual_input: Option<String> = None;
        let mut manual_error: Option<OAuthError> = None;
        let settled = match winner {
            Winner::Redirect(settled) => settled,
            Winner::Manual(prompted) => {
                match prompted {
                    // `:276-279`
                    Ok(input) => manual_input = Some(input),
                    // `:280-283`
                    Err(error) => manual_error = Some(error),
                }
                server.cancel_wait();
                waiter.await
            }
        };

        // `:285` — a rejected wait (exchange failure, login timeout, abort) propagates before the
        // manual result is consulted.
        let settled = settled?;
        // `:286`
        if let Some(error) = manual_error {
            return Err(error);
        }
        // `:287`
        if let Some(credential) = settled {
            return Ok(credential);
        }

        // `:289-290` — `await manualPromise`. Reached only when the listener resolved `null`,
        // which `cancelWait` alone does; when the prompt was the loser it has not settled yet.
        if manual_input.is_none() {
            match manual.await {
                Ok(input) => manual_input = Some(input),
                Err(error) => return Err(error),
            }
        }

        // `:291-292`
        let code = manual_input.as_deref().and_then(parse_authorization_input);
        if !truthy(code.as_deref()) {
            return Err(OAuthError::Failed(MISSING_CODE_MESSAGE.to_string()));
        }
        let code = code.unwrap_or_default();

        // `:293`
        interaction.notify(AuthEvent::Progress {
            message: EXCHANGE_PROGRESS_MESSAGE.to_string(),
        });
        // `:294`
        self.exchange_authorization_code(&code, verifier, interaction.cancel())
            .await
    }

    /// `loginOpenRouter` (`openrouter.ts:242-299`), including its `finally`.
    async fn run_login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        // `:243`
        let pkce = generate_pkce()?;
        // `:244` — a per-login path, so a stale redirect from an earlier attempt 404s.
        let callback_path = format!("/oauth/callback/{}", random_uuid_v4()?);
        // `:245` — `startCallbackServer`; the ephemeral port is `:210`, the deadline `:223`.
        let server: CallbackServer<Credential> = CallbackServer::start(
            CallbackServerConfig::ephemeral(callback_path)
                .with_host(self.bind_host().await)
                .with_timeout(LOGIN_TIMEOUT, LOGIN_TIMEOUT_MESSAGE)
                .with_interaction(interaction),
            OpenRouterCallbackHandler {
                flow: self.clone(),
                verifier: pkce.verifier.clone(),
                cancel: interaction.cancel().cloned(),
            },
        )
        .await?;

        // `:246`
        let manual_abort = CancelToken::new();
        let result = self
            .login_body(
                interaction,
                &server,
                &pkce.challenge,
                &pkce.verifier,
                &manual_abort,
            )
            .await;

        // `:295-298` — `finally { manualAbort.abort(); callback.close(); }`.
        manual_abort.cancel();
        server.close();
        result
    }
}

/// Race a future against the login-wide abort. Upstream hands `signal` straight to `fetch`
/// (`openrouter.ts:101`); Rust has no ambient fetch, so the abort is a `select!`.
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

/// The outer `catch` of `exchangeAuthorizationCode` (`openrouter.ts:109-112`): an aborted login
/// wins, then the exchange deadline, then the underlying error. `fallback` is the message the
/// inner `catch` would otherwise have thrown (`:107`).
fn map_transport_error(
    error: &reqwest::Error,
    cancel: Option<&CancelToken>,
    fallback: Option<&str>,
) -> OAuthError {
    if cancel.is_some_and(CancelToken::is_cancelled) {
        return OAuthError::Cancelled;
    }
    if error.is_timeout() {
        return OAuthError::Failed(EXCHANGE_TIMEOUT_MESSAGE.to_string());
    }
    match fallback {
        Some(message) => OAuthError::Failed(message.to_string()),
        None => OAuthError::Failed(error.to_string()),
    }
}

#[async_trait::async_trait]
impl OAuthAuth for OpenRouterOAuth {
    /// `openrouter.ts:302`
    fn name(&self) -> &str {
        "OpenRouter OAuth"
    }

    /// `openrouter.ts:303`
    fn login_label(&self) -> Option<&str> {
        Some("Sign in with OpenRouter")
    }

    /// 1:1 port of `loginOpenRouter` (`openrouter.ts:242-299`, wired at `:304`).
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        self.run_login(interaction).await
    }

    /// `openrouter.ts:305-307` — the stored value is a permanent API key, so refresh is the
    /// identity. It is **not** a stub: returning the credential unchanged is what keeps
    /// `auth/resolve.rs` from ever discarding a still-valid OpenRouter key.
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        Ok(cred.clone())
    }

    /// `openrouter.ts:308-310` — `{ apiKey: credential.access }`.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        match cred {
            Credential::Oauth { access, .. } => Ok(ModelAuth {
                api_key: Some(access.clone()),
                ..Default::default()
            }),
            Credential::ApiKey { .. } => Err(OAuthError::Failed(
                "OpenRouter OAuth toAuth requires an oauth credential".to_string(),
            )
            .into_auth_error("openrouter")),
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
        // openrouter.ts:20-23
        assert_eq!(AUTHORIZE_URL, "https://openrouter.ai/auth");
        assert_eq!(TOKEN_URL, "https://openrouter.ai/api/v1/auth/keys");
        assert_eq!(LOGIN_TIMEOUT, Duration::from_millis(300_000));
        assert_eq!(TOKEN_EXCHANGE_TIMEOUT, Duration::from_millis(30_000));
        // openrouter.ts:131 — Number.MAX_SAFE_INTEGER === 2**53 - 1.
        assert_eq!(NEVER_EXPIRES, (1i64 << 53) - 1);
        assert_eq!(NEVER_EXPIRES, 9_007_199_254_740_991);
        // openrouter.ts:302-303
        let flow = OpenRouterOAuth::new();
        assert_eq!(flow.name(), "OpenRouter OAuth");
        assert_eq!(flow.login_label(), Some("Sign in with OpenRouter"));
    }

    /// `openrouter.ts:251-256` as `URLSearchParams.toString()` renders it: `://` becomes
    /// `%3A%2F%2F` and every remaining `/` and `:` is escaped. Note there is no `client_id` and
    /// the key is `callback_url`, not `redirect_uri`.
    #[test]
    fn authorization_url_matches_url_search_params() {
        let url = OpenRouterOAuth::new()
            .authorization_url("http://127.0.0.1:41234/oauth/callback/abc-123", "CHAL");
        assert_eq!(
            url,
            concat!(
                "https://openrouter.ai/auth",
                "?callback_url=http%3A%2F%2F127.0.0.1%3A41234%2Foauth%2Fcallback%2Fabc-123",
                "&code_challenge=CHAL",
                "&code_challenge_method=S256",
            )
        );
    }

    // -- parseAuthorizationInput, openrouter.ts:52-67 -----------------------

    #[test]
    fn parses_full_redirect_url() {
        assert_eq!(
            parse_authorization_input("  http://127.0.0.1:8080/oauth/callback/x?code=abc123  "),
            Some("abc123".to_string())
        );
    }

    #[test]
    fn parses_url_with_percent_encoding() {
        assert_eq!(
            parse_authorization_input("https://openrouter.ai/cb?code=a%2Bb"),
            Some("a+b".to_string())
        );
    }

    /// `:56-60` — the URL branch `return`s. A parseable URL with no `code` yields `undefined`; it
    /// does NOT fall through to the bare-value branch and hand back the whole URL.
    #[test]
    fn url_without_code_yields_none_rather_than_the_url() {
        assert_eq!(
            parse_authorization_input("http://127.0.0.1:8080/oauth/callback/x"),
            None
        );
    }

    #[test]
    fn url_fragment_is_not_searchable() {
        assert_eq!(parse_authorization_input("http://localhost/cb#code=nope"), None);
    }

    #[test]
    fn parses_bare_query_string() {
        assert_eq!(
            parse_authorization_input("code=abc&state=xyz"),
            Some("abc".to_string())
        );
    }

    #[test]
    fn parses_bare_code() {
        assert_eq!(
            parse_authorization_input("just-a-code"),
            Some("just-a-code".to_string())
        );
    }

    #[test]
    fn empty_input_yields_none() {
        assert_eq!(parse_authorization_input("   "), None);
    }

    /// `searchParams.get("code")` is `""` for `?code=`, which `if (!code)` (`:292`) rejects.
    #[test]
    fn empty_code_param_survives_as_empty_string_and_is_falsy() {
        assert_eq!(
            parse_authorization_input("code=&other=1"),
            Some(String::new())
        );
        assert!(!truthy(Some("")));
        // Mirror: a non-empty code is truthy, so the check above is not vacuous.
        assert!(truthy(parse_authorization_input("code=ok").as_deref()));
    }

    // -- errorDetail, openrouter.ts:69-78 -----------------------------------

    fn object(json: &str) -> serde_json::Map<String, serde_json::Value> {
        match serde_json::from_str::<serde_json::Value>(json) {
            Ok(serde_json::Value::Object(map)) => map,
            other => panic!("fixture is not a JSON object: {other:?}"),
        }
    }

    #[test]
    fn error_detail_follows_upstream_precedence() {
        // `:70` wins over everything below it.
        assert_eq!(
            error_detail(&object(
                r#"{"error_description":"desc","message":"msg","error":"err"}"#
            )),
            Some("desc".to_string())
        );
        // `:71`
        assert_eq!(
            error_detail(&object(r#"{"message":"msg","error":"err"}"#)),
            Some("msg".to_string())
        );
        // `:72`
        assert_eq!(
            error_detail(&object(r#"{"error":"err"}"#)),
            Some("err".to_string())
        );
        // `:73-76`
        assert_eq!(
            error_detail(&object(r#"{"error":{"message":"nested"}}"#)),
            Some("nested".to_string())
        );
        // `:73` — `Array.isArray` is excluded.
        assert_eq!(error_detail(&object(r#"{"error":["a"]}"#)), None);
        // `:77`
        assert_eq!(error_detail(&object(r#"{"unrelated":1}"#)), None);
        // Non-string fields are ignored at every rung (`typeof ... === "string"`).
        assert_eq!(error_detail(&object(r#"{"error_description":7}"#)), None);
    }

    #[test]
    fn detail_suffix_matches_the_js_template() {
        // `:120` — `${detail ? `: ${detail}` : ""}`.
        assert_eq!(detail_suffix(Some("boom")), ": boom");
        assert_eq!(detail_suffix(Some("")), "");
        assert_eq!(detail_suffix(None), "");
    }

    // -- a loopback token endpoint ------------------------------------------

    /// A single-shot loopback token endpoint. Never touches the network.
    struct FakeTokenServer {
        url: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl FakeTokenServer {
        fn start(status: u16, body: &'static str) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let sink = Arc::clone(&requests);
            let handle = std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buf = [0u8; 8192];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let raw = String::from_utf8_lossy(&buf[..n]).to_string();
                    let (head, payload) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
                    sink.lock()
                        .unwrap()
                        .push((head.to_string(), payload.to_string()));
                    let reason = if (200..300).contains(&status) {
                        "OK"
                    } else {
                        "Bad Request"
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                }
            });
            FakeTokenServer {
                url: format!("http://127.0.0.1:{port}/api/v1/auth/keys"),
                requests,
                handle: Some(handle),
            }
        }

        fn recorded(&mut self) -> (String, serde_json::Value) {
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
            let requests = self.requests.lock().unwrap();
            let (head, payload) = requests.first().cloned().unwrap();
            (head, serde_json::from_str(&payload).unwrap())
        }
    }

    fn strategy_for(token_url: &str) -> OpenRouterOAuth {
        OpenRouterOAuth::with_endpoints(AUTHORIZE_URL, token_url, "127.0.0.1")
    }

    #[tokio::test]
    async fn exchange_sends_upstream_request_shape_and_never_expires() {
        let mut server = FakeTokenServer::start(200, r#"{"key":"sk-or-v1-abc"}"#);
        let flow = strategy_for(&server.url);
        let cred = flow
            .exchange_authorization_code("CODE", "VERIF", None)
            .await
            .unwrap();

        let (head, body) = server.recorded();
        assert!(head.starts_with("POST /api/v1/auth/keys HTTP/1.1"), "{head}");
        // openrouter.ts:99
        let lower = head.to_lowercase();
        assert!(lower.contains("accept: application/json"), "{head}");
        assert!(lower.contains("content-type: application/json"), "{head}");
        // openrouter.ts:100 — no client_id, no redirect_uri, no grant_type.
        assert_eq!(
            body,
            serde_json::json!({
                "code": "CODE",
                "code_verifier": "VERIF",
                "code_challenge_method": "S256",
            })
        );

        // openrouter.ts:127-132
        match cred {
            Credential::Oauth {
                access,
                refresh,
                expires,
                ..
            } => {
                assert_eq!(access, "sk-or-v1-abc");
                assert_eq!(refresh, "");
                assert_eq!(expires, 9_007_199_254_740_991);
            }
            other => panic!("expected oauth credential, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn exchange_reports_http_failure_with_detail() {
        // openrouter.ts:118-121, detail from `:70`.
        let server = FakeTokenServer::start(400, r#"{"error_description":"bad code"}"#);
        let err = strategy_for(&server.url)
            .exchange_authorization_code("CODE", "VERIF", None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "OpenRouter OAuth key exchange failed (HTTP 400): bad code"
        );
    }

    #[tokio::test]
    async fn exchange_reports_http_failure_without_detail_when_body_is_not_json() {
        // `:103-108` — a non-2xx whose body will not parse leaves `body = {}`, so no `: detail`.
        let server = FakeTokenServer::start(503, "<html>gateway</html>");
        let err = strategy_for(&server.url)
            .exchange_authorization_code("CODE", "VERIF", None)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "OpenRouter OAuth key exchange failed (HTTP 503)"
        );
    }

    #[tokio::test]
    async fn exchange_rejects_invalid_json_on_success() {
        // openrouter.ts:107
        let server = FakeTokenServer::start(200, "not json");
        let err = strategy_for(&server.url)
            .exchange_authorization_code("CODE", "VERIF", None)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "OpenRouter OAuth returned invalid JSON");
    }

    #[tokio::test]
    async fn exchange_rejects_a_json_array_body() {
        // `:105` — only a non-array object is adopted, so `body.key` is missing.
        let server = FakeTokenServer::start(200, r#"["key"]"#);
        let err = strategy_for(&server.url)
            .exchange_authorization_code("CODE", "VERIF", None)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "OpenRouter OAuth response carries no \"key\"");
    }

    #[tokio::test]
    async fn exchange_rejects_missing_and_empty_key() {
        // openrouter.ts:123-125 — `typeof body.key !== "string" || body.key.length === 0`.
        for body in [r#"{}"#, r#"{"key":""}"#, r#"{"key":7}"#] {
            let server = FakeTokenServer::start(200, body);
            let err = strategy_for(&server.url)
                .exchange_authorization_code("CODE", "VERIF", None)
                .await
                .unwrap_err();
            assert_eq!(
                err.to_string(),
                "OpenRouter OAuth response carries no \"key\"",
                "body={body}"
            );
        }
        // Mirror: a well-formed key still succeeds, so the loop above is not vacuous.
        let server = FakeTokenServer::start(200, r#"{"key":"sk-or-v1-ok"}"#);
        assert!(
            strategy_for(&server.url)
                .exchange_authorization_code("CODE", "VERIF", None)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn exchange_refuses_to_start_once_cancelled() {
        // openrouter.ts:85
        let server = FakeTokenServer::start(200, r#"{"key":"never-used"}"#);
        let token = CancelToken::new();
        token.cancel();
        let err = strategy_for(&server.url)
            .exchange_authorization_code("CODE", "VERIF", Some(&token))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Login cancelled");
    }

    // -- the whole login, over loopback -------------------------------------

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

    /// Block until the flow has published its authorize URL, exactly as a TUI would.
    async fn await_auth_url(interaction: &ScriptedInteraction) -> String {
        loop {
            let found = interaction.events().into_iter().find_map(|event| match event {
                AuthEvent::AuthUrl { url, .. } => Some(url),
                _ => None,
            });
            if let Some(url) = found {
                return url;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn callback_url_of(authorize: &str) -> String {
        let (_, query) = authorize.split_once('?').expect("authorize URL has a query");
        parse_query(query)
            .into_iter()
            .find(|(k, _)| k == "callback_url")
            .map(|(_, v)| v)
            .expect("authorize URL carries callback_url")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn browser_redirect_completes_the_login() {
        let server = FakeTokenServer::start(200, r#"{"key":"sk-or-v1-redirect"}"#);
        let flow = strategy_for(&server.url);
        // No scripted answer: the manual prompt blocks, so the redirect wins the race.
        let interaction = Arc::new(ScriptedInteraction::new(Vec::new()).blocking_when_empty());

        let login = {
            let flow = flow.clone();
            let interaction = Arc::clone(&interaction);
            tokio::spawn(async move { flow.login(interaction.as_ref()).await })
        };

        let authorize = await_auth_url(&interaction).await;
        assert!(authorize.starts_with("https://openrouter.ai/auth?callback_url="));
        assert!(authorize.contains("&code_challenge_method=S256"));
        let callback_url = callback_url_of(&authorize);
        // openrouter.ts:210/232 — loopback, ephemeral port, per-login UUID path.
        assert!(callback_url.starts_with("http://127.0.0.1:"), "{callback_url}");
        assert!(callback_url.contains("/oauth/callback/"), "{callback_url}");

        let response =
            tokio::task::spawn_blocking(move || browser_get(&format!("{callback_url}?code=CB")))
                .await
                .unwrap();
        // openrouter.ts:198 + `sendHtml`'s `cache-control: no-store` (`:48`).
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.to_lowercase().contains("cache-control: no-store"));
        assert!(response.contains("Signed in to OpenRouter. You may now close this page."));

        let cred = login.await.unwrap().expect("login succeeds");
        match &cred {
            Credential::Oauth {
                access, expires, ..
            } => {
                assert_eq!(access, "sk-or-v1-redirect");
                assert_eq!(*expires, 9_007_199_254_740_991);
            }
            other => panic!("expected oauth credential, got {other:?}"),
        }

        // openrouter.ts:258-261 — the progress line names the callback URL.
        let progress = interaction
            .events()
            .into_iter()
            .find_map(|event| match event {
                AuthEvent::Progress { message } => Some(message),
                _ => None,
            })
            .expect("a progress event");
        assert!(
            progress.starts_with("Listening for OpenRouter OAuth callback on http://127.0.0.1:"),
            "{progress}"
        );

        // openrouter.ts:308-310
        let auth = flow.to_auth(&cred).await.unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("sk-or-v1-redirect"));
        // openrouter.ts:305-307 — refresh is the identity.
        let refreshed = flow.refresh(&cred).await.unwrap();
        assert_eq!(
            serde_json::to_value(&refreshed).unwrap(),
            serde_json::to_value(&cred).unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pasted_redirect_url_completes_the_login() {
        let server = FakeTokenServer::start(200, r#"{"key":"sk-or-v1-pasted"}"#);
        let flow = strategy_for(&server.url);
        // The prompt answers immediately, so the paste wins and `cancelWait` (`:278`) resolves the
        // listener with `null` — the `:289-294` branch.
        let interaction = Arc::new(ScriptedInteraction::new(vec![Ok(
            "http://127.0.0.1:1/oauth/callback/z?code=PASTED".to_string(),
        )]));

        let cred = flow.login(interaction.as_ref()).await.expect("login succeeds");
        match &cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "sk-or-v1-pasted"),
            other => panic!("expected oauth credential, got {other:?}"),
        }

        // openrouter.ts:269-275 — the prompt is a `manual_code` whose placeholder is the callback
        // URL the listener actually bound.
        let prompts = interaction.prompts();
        let prompt = prompts.first().expect("a manual_code prompt");
        assert_eq!(
            prompt.kind,
            Some(crate::auth::oauth::interaction::AuthPromptKind::ManualCode)
        );
        assert_eq!(
            prompt.message,
            "Complete sign-in in your browser, or paste the authorization code / redirect URL here:"
        );
        assert!(
            prompt
                .placeholder
                .as_deref()
                .is_some_and(|p| p.starts_with("http://127.0.0.1:")),
            "{:?}",
            prompt.placeholder
        );

        // openrouter.ts:293
        assert!(
            interaction.events().contains(&AuthEvent::Progress {
                message: "Exchanging authorization code for an API key...".to_string(),
            })
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pasted_garbage_reports_missing_authorization_code() {
        // `:291-292` — a paste that parses to `""` is still missing.
        let server = FakeTokenServer::start(200, r#"{"key":"never-used"}"#);
        let flow = strategy_for(&server.url);
        let interaction = Arc::new(ScriptedInteraction::new(vec![Ok("   ".to_string())]));
        let err = flow.login(interaction.as_ref()).await.unwrap_err();
        assert_eq!(err.to_string(), "Missing authorization code");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn callback_error_parameter_fails_the_login() {
        let server = FakeTokenServer::start(200, r#"{"key":"never-used"}"#);
        let flow = strategy_for(&server.url);
        let interaction = Arc::new(ScriptedInteraction::new(Vec::new()).blocking_when_empty());

        let login = {
            let flow = flow.clone();
            let interaction = Arc::clone(&interaction);
            tokio::spawn(async move { flow.login(interaction.as_ref()).await })
        };
        let callback_url = callback_url_of(&await_auth_url(&interaction).await);

        let response = tokio::task::spawn_blocking(move || {
            browser_get(&format!(
                "{callback_url}?error=access_denied&error_description=User%20said%20no"
            ))
        })
        .await
        .unwrap();
        // openrouter.ts:184
        assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{response}");
        assert!(response.contains("OpenRouter authorization was denied."));
        assert!(response.contains("User said no"));

        // openrouter.ts:185
        let err = login.await.unwrap().unwrap_err();
        assert_eq!(
            err.to_string(),
            "OpenRouter authorization failed: User said no"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn callback_error_without_description_falls_back_to_the_error_code() {
        let server = FakeTokenServer::start(200, r#"{"key":"never-used"}"#);
        let flow = strategy_for(&server.url);
        let interaction = Arc::new(ScriptedInteraction::new(Vec::new()).blocking_when_empty());
        let login = {
            let flow = flow.clone();
            let interaction = Arc::clone(&interaction);
            tokio::spawn(async move { flow.login(interaction.as_ref()).await })
        };
        let callback_url = callback_url_of(&await_auth_url(&interaction).await);
        let _ = tokio::task::spawn_blocking(move || {
            browser_get(&format!("{callback_url}?error=server_error"))
        })
        .await
        .unwrap();
        // openrouter.ts:183 — `error_description` absent, so `?? oauthError`.
        assert_eq!(
            login.await.unwrap().unwrap_err().to_string(),
            "OpenRouter authorization failed: server_error"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn callback_without_a_code_keeps_listening() {
        let server = FakeTokenServer::start(200, r#"{"key":"sk-or-v1-second"}"#);
        let flow = strategy_for(&server.url);
        let interaction = Arc::new(ScriptedInteraction::new(Vec::new()).blocking_when_empty());
        let login = {
            let flow = flow.clone();
            let interaction = Arc::clone(&interaction);
            tokio::spawn(async move { flow.login(interaction.as_ref()).await })
        };
        let callback_url = callback_url_of(&await_auth_url(&interaction).await);

        // openrouter.ts:190-193 — answered, but the wait is NOT settled.
        let first = {
            let callback_url = callback_url.clone();
            tokio::task::spawn_blocking(move || browser_get(&callback_url))
                .await
                .unwrap()
        };
        assert!(first.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{first}");
        assert!(first.contains("OpenRouter returned no authorization code."));

        // A second, good redirect still completes the login.
        let second = tokio::task::spawn_blocking(move || {
            browser_get(&format!("{callback_url}?code=LATER"))
        })
        .await
        .unwrap();
        assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
        match login.await.unwrap().expect("login succeeds") {
            Credential::Oauth { access, .. } => assert_eq!(access, "sk-or-v1-second"),
            other => panic!("expected oauth credential, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_failed_exchange_shows_the_browser_a_502_and_fails_the_login() {
        let server = FakeTokenServer::start(400, r#"{"error":"invalid_grant"}"#);
        let flow = strategy_for(&server.url);
        let interaction = Arc::new(ScriptedInteraction::new(Vec::new()).blocking_when_empty());
        let login = {
            let flow = flow.clone();
            let interaction = Arc::clone(&interaction);
            tokio::spawn(async move { flow.login(interaction.as_ref()).await })
        };
        let callback_url = callback_url_of(&await_auth_url(&interaction).await);

        let response =
            tokio::task::spawn_blocking(move || browser_get(&format!("{callback_url}?code=BAD")))
                .await
                .unwrap();
        // openrouter.ts:202
        assert!(response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{response}");
        assert!(response.contains("OpenRouter key exchange failed."));
        // openrouter.ts:120 with the `:72` detail rung.
        assert!(
            response.contains("OpenRouter OAuth key exchange failed (HTTP 400): invalid_grant"),
            "{response}"
        );
        assert_eq!(
            login.await.unwrap().unwrap_err().to_string(),
            "OpenRouter OAuth key exchange failed (HTTP 400): invalid_grant"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn aborting_the_login_reports_login_cancelled() {
        let server = FakeTokenServer::start(200, r#"{"key":"never-used"}"#);
        let flow = strategy_for(&server.url);
        let token = CancelToken::new();
        let interaction = Arc::new(
            ScriptedInteraction::new(Vec::new())
                .blocking_when_empty()
                .with_cancel(token.clone()),
        );
        let login = {
            let interaction = Arc::clone(&interaction);
            tokio::spawn(async move { flow.login(interaction.as_ref()).await })
        };
        let _ = await_auth_url(&interaction).await;
        token.cancel();
        // openrouter.ts:217 — `finish({ error: new Error("Login cancelled") })`.
        assert_eq!(
            login.await.unwrap().unwrap_err().to_string(),
            "Login cancelled"
        );
    }
}
