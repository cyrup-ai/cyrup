//! Anthropic OAuth flow (Claude Pro/Max) — 1:1 port of pi v0.83.0
//! `packages/ai/src/auth/oauth/anthropic.ts` (350 lines).
//!
//! This is the subscription login: a PKCE authorization-code flow whose redirect lands on the
//! loopback callback listener, raced against a manual-paste prompt for the case where the browser
//! runs on another machine. It also owns the refresh-token exchange that
//! [`crate::auth::resolve`] drives under the credential-store lock.
//!
//! ## Provenance
//!
//! | this module | `anthropic.ts` |
//! |---|---|
//! | [`CLIENT_ID`] / [`AUTHORIZE_URL`] / [`TOKEN_URL`] / [`SCOPES`] / [`CALLBACK_PORT`] | `:29-37` |
//! | [`parse_authorization_input`] | `parseAuthorizationInput`, `:52-79` |
//! | [`format_error_details`] | `formatErrorDetails`, `:81-97` |
//! | [`AnthropicCallbackHandler`] | the `createServer` request handler, `:114-148` |
//! | [`post_json`] | `postJson`, `:170-188` |
//! | [`AnthropicOAuth::exchange_authorization_code`] | `exchangeAuthorizationCode`, `:190-227` |
//! | [`AnthropicOAuth::login`] | `loginAnthropic`, `:229-303` |
//! | [`AnthropicOAuth::refresh_token`] | `refreshAnthropicToken`, `:308-340` |
//! | `impl OAuthAuth for AnthropicOAuth` | `anthropicOAuth`, `:342-350` |
//!
//! The listener, PKCE, the callback pages and `URLSearchParams` come from the shared [`super`]
//! substrate; upstream hand-rolls a listener per flow, and `anthropic.ts:99-168` is one of the
//! four copies [`super::callback`] factors out.
//!
//! ## Mechanism divergences (Rust forces these; behaviour is unchanged)
//!
//! * **Callback server.** Upstream calls `node:http.createServer` directly (`:8`, `:100`), and its
//!   `getNodeApis()` guard (`:38-50`) rejects browser environments. Here the listener is
//!   [`super::callback::CallbackServer`] (a `std::net::TcpListener` accept thread, because this
//!   crate's `tokio` carries no `net` feature) and the browser guard is dropped: a Rust build is
//!   never a browser. The 404-on-foreign-route branch (`:117-120`) lives in that shared server.
//! * **Cancellation.** Upstream threads an `AbortSignal` into the manual prompt and calls
//!   `manualAbort.abort()` in `finally` (`:232`, `:261`, `:300`); here that signal is a
//!   [`CancelToken`] on `AuthPrompt::cancel`, fired on the branch where the redirect wins.
//!   `server.server.close()` (`:301`) is the [`super::callback::CallbackServer`] drop.
//! * **`formatErrorDetails`.** JS `Error` has `name`, `stack`, `code` and `errno` (`:81-97`);
//!   Rust's [`std::error::Error`] has none of them. [`format_error_details`] emits
//!   `Error: {Display}` — `Error` being the name of every `new Error(...)` this upstream module
//!   throws — and recurses through [`std::error::Error::source`] for the `cause=` chain, exactly
//!   as upstream recurses into `error.cause`.
//! * **Error type.** Upstream `throw new Error(msg)`; here the message is the `Display` of an
//!   [`OAuthError`], which `into_auth_error` folds into the crate taxonomy (func-01 R-01-017).
//!   Every message string is preserved verbatim.
//! * **Endpoint override.** Upstream's tests stub the ambient `fetch`; Rust has no ambient fetch,
//!   so the endpoints are struct fields ([`AnthropicOAuth::with_endpoints`]) defaulting to the
//!   upstream constants. Production callers use [`AnthropicOAuth::new`] and get `:30-35` exactly.

use super::callback::{
    CallbackControl, CallbackHandler, CallbackOutcome, CallbackReply, CallbackRequest,
    CallbackServer, CallbackServerConfig, callback_host,
};
use super::interaction::{AuthEvent, AuthInteraction, AuthPrompt};
use super::pkce::generate_pkce;
use super::query::{encode_query, parse_query};
use super::{OAuthError, now_ms, oauth_credential};
use crate::auth::OAuthAuth;
use crate::auth::types::{Credential, EnvAuthContext, ModelAuth};
use crate::error::AuthError;
use cyrup_core::CancelToken;
use std::fmt::Write as _;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants — anthropic.ts:29-37
// ---------------------------------------------------------------------------

/// The public OAuth client id. Upstream writes it as `atob("OWQxYzI1MGEt…")` (`anthropic.ts:28-29`);
/// the base64 wrapper is cosmetic obfuscation of a non-secret public client id. Rust has no `atob`,
/// and decoding at load time would mean fallible handling of a compile-time-known constant, so the
/// decoded value is the constant. `client_id_matches_upstream_base64` proves the two are equal.
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

/// `anthropic.ts:29` — the exact base64 upstream decodes, kept so the equality test asserts
/// against an upstream fixture rather than restating [`CLIENT_ID`].
#[cfg(test)]
const CLIENT_ID_BASE64: &str = "OWQxYzI1MGEtZTYxYi00NGQ5LTg4ZWQtNTk0NGQxOTYyZjVl";

/// `anthropic.ts:30`.
pub const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
/// `anthropic.ts:31`.
pub const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// `anthropic.ts:33` — a fixed, pre-registered port, so a second concurrent login fails to bind.
pub const CALLBACK_PORT: u16 = 53692;
/// `anthropic.ts:34`.
pub const CALLBACK_PATH: &str = "/callback";
/// `anthropic.ts:35` — the redirect URI is advertised on `localhost` even though the listener
/// binds `CALLBACK_HOST` (`127.0.0.1` unless `*_OAUTH_CALLBACK_HOST` says otherwise).
pub const ADVERTISE_HOST: &str = "localhost";
/// `anthropic.ts:36-37`. Space-separated; the urlencoded serializer turns the spaces into `+`.
pub const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// `anthropic.ts:252-253`.
const AUTH_URL_INSTRUCTIONS: &str = "Complete login in your browser. If the browser is on another machine, paste the final redirect URL here.";
/// `anthropic.ts:259`.
const MANUAL_PROMPT_MESSAGE: &str =
    "Complete login in your browser, or paste the authorization code / redirect URL here:";
/// `anthropic.ts:297`.
const EXCHANGE_PROGRESS_MESSAGE: &str = "Exchanging authorization code for tokens...";

/// `anthropic.ts:178` — `AbortSignal.timeout(30_000)` on the token endpoint.
const POST_TIMEOUT: Duration = Duration::from_secs(30);

/// The five-minute safety margin subtracted from every issued deadline (`anthropic.ts:225`, `:338`).
const EXPIRY_SKEW_MS: i64 = 5 * 60 * 1000;

/// JS truthiness for an optional string: `undefined` and `""` are both falsy. Upstream depends on
/// this at `:127`, `:133`, `:284`, `:295` and `:296` — `searchParams.get()` yields `""` for a bare
/// `?code=`, which has to read as missing.
fn truthy(value: Option<&str>) -> bool {
    value.is_some_and(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// parseAuthorizationInput — anthropic.ts:52-79
// ---------------------------------------------------------------------------

/// What the user pasted, teased apart. Both fields keep JS's `undefined`-vs-`""` distinction,
/// because the caller's checks turn on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedAuthorizationInput {
    pub code: Option<String>,
    pub state: Option<String>,
}

/// The query component of an absolute URL — what `new URL(value).searchParams` reads — or `None`
/// when `new URL(value)` would throw.
///
/// Approximates the WHATWG parser's scheme-start state: ASCII alpha, then
/// alpha/digit/`+`/`-`/`.`, then `:`. That is the whole discrimination
/// [`parse_authorization_input`] needs, since an authorization code carries no scheme.
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
    // A fragment is not part of `searchParams`, and a `#` before any `?` means there is no query.
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

/// 1:1 port of `parseAuthorizationInput` (`anthropic.ts:52-79`). Accepts, in order: a full
/// redirect URL, `code#state`, a raw `code=…&state=…` query string, or a bare code.
pub fn parse_authorization_input(input: &str) -> ParsedAuthorizationInput {
    // `:53-54`
    let value = input.trim();
    if value.is_empty() {
        return ParsedAuthorizationInput::default();
    }

    // `:56-63` — a parseable URL wins, even when it carries neither parameter.
    if let Some(query) = absolute_url_query(value) {
        return ParsedAuthorizationInput {
            code: query_get(query, "code"),
            state: query_get(query, "state"),
        };
    }

    // `:66-69` — `value.split("#", 2)`. JS's limit *truncates*, so `a#b#c` yields `a` and `b`.
    if value.contains('#') {
        let mut parts = value.split('#');
        return ParsedAuthorizationInput {
            code: parts.next().map(str::to_string),
            state: parts.next().map(str::to_string),
        };
    }

    // `:71-77`
    if value.contains("code=") {
        return ParsedAuthorizationInput {
            code: query_get(value, "code"),
            state: query_get(value, "state"),
        };
    }

    // `:78`
    ParsedAuthorizationInput {
        code: Some(value.to_string()),
        state: None,
    }
}

// ---------------------------------------------------------------------------
// formatErrorDetails — anthropic.ts:81-97
// ---------------------------------------------------------------------------

/// Port of `formatErrorDetails` (`anthropic.ts:81-97`). `Error: {message}` mirrors JS's
/// `${error.name}: ${error.message}` for the plain `Error`s this module throws; `code=`, `errno=`
/// and `stack=` have no Rust analogue and are omitted, while `cause=` recurses through
/// [`std::error::Error::source`] exactly as upstream recurses into `error.cause`.
pub fn format_error_details(error: &(dyn std::error::Error + 'static)) -> String {
    let mut details = format!("Error: {error}");
    if let Some(source) = error.source() {
        let _ = write!(details, "; cause={}", format_error_details(source));
    }
    details
}

// ---------------------------------------------------------------------------
// postJson — anthropic.ts:170-188
// ---------------------------------------------------------------------------

/// The failure modes of [`post_json`]. `Status`'s `Display` is upstream's `:184` message verbatim.
#[derive(Debug)]
enum PostError {
    Transport(reqwest::Error),
    Status {
        status: u16,
        url: String,
        body: String,
    },
}

impl std::fmt::Display for PostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // `anthropic.ts:184`
            PostError::Status { status, url, body } => write!(
                f,
                "HTTP request failed. status={status}; url={url}; body={body}"
            ),
            PostError::Transport(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PostError::Transport(e) => Some(e),
            PostError::Status { .. } => None,
        }
    }
}

/// 1:1 port of `postJson` (`anthropic.ts:170-188`): JSON body, `Accept: application/json`, a 30 s
/// deadline; the response text is returned on 2xx and raised on anything else.
async fn post_json(
    client: &reqwest::Client,
    url: &str,
    body: &serde_json::Value,
) -> Result<String, PostError> {
    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .timeout(POST_TIMEOUT)
        .json(body)
        .send()
        .await
        .map_err(PostError::Transport)?;

    let status = response.status();
    let response_body = response.text().await.map_err(PostError::Transport)?;

    // `response.ok` is 200-299 (`:183`).
    if !status.is_success() {
        return Err(PostError::Status {
            status: status.as_u16(),
            url: url.to_string(),
            body: response_body,
        });
    }

    Ok(response_body)
}

/// The token endpoint's success payload (`anthropic.ts:212`, `:320-327`). `scope` is accepted and
/// ignored, as upstream does.
#[derive(Debug, serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
}

impl TokenResponse {
    /// `anthropic.ts:221-226` / `:334-339` — `Date.now() + expires_in * 1000 - 5 * 60 * 1000`.
    fn into_credential(self) -> Credential {
        oauth_credential(
            self.access_token,
            self.refresh_token,
            now_ms() + self.expires_in * 1000 - EXPIRY_SKEW_MS,
        )
    }
}

// ---------------------------------------------------------------------------
// The callback handler — anthropic.ts:114-148
// ---------------------------------------------------------------------------

/// A `code`/`state` pair delivered by the browser redirect (`anthropic.ts:18`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationCallback {
    pub code: String,
    pub state: String,
}

/// The Anthropic-specific half of the callback server (`anthropic.ts:114-148`).
///
/// Every rejection branch replies **without** settling the wait, exactly as upstream's handler
/// `res.end()`s without reaching `settleWait`: the browser sees the error page and the listener
/// keeps waiting for a good redirect.
pub struct AnthropicCallbackHandler {
    /// `expectedState` (`anthropic.ts:99`), which is the PKCE verifier (`:231`).
    expected_state: String,
}

#[async_trait::async_trait]
impl CallbackHandler for AnthropicCallbackHandler {
    type Value = AuthorizationCallback;

    async fn handle(
        &self,
        request: CallbackRequest,
        _control: CallbackControl,
    ) -> CallbackOutcome<Self::Value> {
        // `:122-124`
        let code = request.param("code");
        let state = request.param("state");
        let error = request.param("error");

        // `:126-131` — JS truthiness, so a bare `?error=` is not an error.
        if truthy(error) {
            let error = error.unwrap_or_default();
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(
                    400,
                    "Anthropic authentication did not complete.",
                    Some(&format!("Error: {error}")),
                ),
            };
        }

        // `:133-136`
        if !truthy(code) || !truthy(state) {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(400, "Missing code or state parameter.", None),
            };
        }
        let code = code.unwrap_or_default().to_string();
        let state = state.unwrap_or_default().to_string();

        // `:138-141`
        if state != self.expected_state {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(400, "State mismatch.", None),
            };
        }

        // `:143-147`
        CallbackOutcome::Complete {
            reply: CallbackReply::success(
                "Anthropic authentication completed. You can close this window.",
            ),
            value: AuthorizationCallback { code, state },
        }
    }
}

// ---------------------------------------------------------------------------
// anthropicOAuth — anthropic.ts:342-350
// ---------------------------------------------------------------------------

/// The Anthropic (Claude Pro/Max) subscription OAuth strategy — upstream's `anthropicOAuth`
/// (`anthropic.ts:342-350`).
#[derive(Clone, Debug)]
pub struct AnthropicOAuth {
    authorize_url: String,
    token_url: String,
    /// `None` resolves `CYRUP_OAUTH_CALLBACK_HOST`/`PI_OAUTH_CALLBACK_HOST` at login time, which
    /// is what upstream's module-level `CALLBACK_HOST` const does (`anthropic.ts:32`).
    callback_host: Option<String>,
    callback_port: u16,
}

impl Default for AnthropicOAuth {
    fn default() -> Self {
        Self::new()
    }
}

impl AnthropicOAuth {
    /// The production strategy — upstream's `:30-35` constants.
    pub fn new() -> Self {
        AnthropicOAuth {
            authorize_url: AUTHORIZE_URL.to_string(),
            token_url: TOKEN_URL.to_string(),
            callback_host: None,
            callback_port: CALLBACK_PORT,
        }
    }

    /// Point the flow at a different authorize/token endpoint and callback bind. Exists only
    /// because Rust has no ambient `fetch` for a test to stub the way upstream's would; production
    /// code uses [`AnthropicOAuth::new`]. A `callback_port` of `0` binds an ephemeral port.
    pub fn with_endpoints(
        authorize_url: impl Into<String>,
        token_url: impl Into<String>,
        callback_host: impl Into<String>,
        callback_port: u16,
    ) -> Self {
        AnthropicOAuth {
            authorize_url: authorize_url.into(),
            token_url: token_url.into(),
            callback_host: Some(callback_host.into()),
            callback_port,
        }
    }

    /// The browser URL the user must open — `anthropic.ts:239-251`. Parameter order is upstream's
    /// insertion order, which `URLSearchParams.toString()` preserves. Note `state` is the PKCE
    /// verifier itself (`:247`), which is also what the callback handler matches on (`:231`).
    pub fn authorization_url(&self, challenge: &str, verifier: &str, redirect_uri: &str) -> String {
        let params = encode_query([
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri),
            ("scope", SCOPES),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", verifier),
        ]);
        format!("{}?{params}", self.authorize_url)
    }

    /// PROV-047: proxy-aware, per target. `build_client()` consulted neither the ported resolver
    /// nor the `httpProxy` setting, so every OAuth token exchange and silent refresh bypassed a
    /// configured proxy while provider streaming used it.
    async fn client(&self, target_url: &str) -> Result<reqwest::Client, OAuthError> {
        crate::stream::sse::build_client_for(target_url)
            .await
            .map_err(|e| OAuthError::Failed(e.to_string()))
    }

    /// 1:1 port of `exchangeAuthorizationCode` (`anthropic.ts:190-227`).
    async fn exchange_authorization_code(
        &self,
        code: &str,
        state: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<Credential, OAuthError> {
        let client = self.client(&self.token_url).await?;
        // `:198-205`
        let body = serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "state": state,
            "redirect_uri": redirect_uri,
            "code_verifier": verifier,
        });

        let response_body = match post_json(&client, &self.token_url, &body).await {
            Ok(b) => b,
            // `:206-210`
            Err(e) => {
                return Err(OAuthError::Failed(format!(
                    "Token exchange request failed. url={}; redirect_uri={redirect_uri}; response_type=authorization_code; details={}",
                    self.token_url,
                    format_error_details(&e)
                )));
            }
        };

        match serde_json::from_str::<TokenResponse>(&response_body) {
            Ok(token) => Ok(token.into_credential()),
            // `:215-219`
            Err(e) => Err(OAuthError::Failed(format!(
                "Token exchange returned invalid JSON. url={}; body={response_body}; details={}",
                self.token_url,
                format_error_details(&e)
            ))),
        }
    }

    /// 1:1 port of `refreshAnthropicToken` (`anthropic.ts:308-340`).
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<Credential, OAuthError> {
        let client = self.client(&self.token_url).await?;
        // `:311-315`
        let body = serde_json::json!({
            "grant_type": "refresh_token",
            "client_id": CLIENT_ID,
            "refresh_token": refresh_token,
        });

        let response_body = match post_json(&client, &self.token_url, &body).await {
            Ok(b) => b,
            // `:316-318`
            Err(e) => {
                return Err(OAuthError::Failed(format!(
                    "Anthropic token refresh request failed. url={}; details={}",
                    self.token_url,
                    format_error_details(&e)
                )));
            }
        };

        match serde_json::from_str::<TokenResponse>(&response_body) {
            Ok(token) => Ok(token.into_credential()),
            // `:328-332`
            Err(e) => Err(OAuthError::Failed(format!(
                "Anthropic token refresh returned invalid JSON. url={}; body={response_body}; details={}",
                self.token_url,
                format_error_details(&e)
            ))),
        }
    }

    /// The listener's bind host: upstream's `CALLBACK_HOST` (`anthropic.ts:32`).
    async fn bind_host(&self) -> String {
        match &self.callback_host {
            Some(host) => host.clone(),
            None => callback_host(&EnvAuthContext, None).await,
        }
    }

    /// The body of [`OAuthAuth::login`]; see that impl for the port notes.
    async fn run_login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        // `:230`
        let pkce = generate_pkce()?;

        // `:231` — the verifier doubles as the OAuth state. The port is fixed and pre-registered,
        // so a second concurrent login surfaces as `OAuthError::Listen`, which is upstream's
        // `server.on("error", reject)` (`:150-152`); there is no manual-paste fallback for it.
        let config = CallbackServerConfig::fixed(self.callback_port, CALLBACK_PATH)
            .with_host(self.bind_host().await)
            .advertising(ADVERTISE_HOST)
            .with_interaction(interaction);
        let server = CallbackServer::start(
            config,
            AnthropicCallbackHandler {
                expected_state: pkce.verifier.clone(),
            },
        )
        .await?;
        // `:35`/`:160` — `http://localhost:{port}/callback`.
        let redirect_uri = server.redirect_uri().to_string();

        // `:249-254`
        interaction.notify(AuthEvent::AuthUrl {
            url: self.authorization_url(&pkce.challenge, &pkce.verifier, &redirect_uri),
            instructions: Some(AUTH_URL_INSTRUCTIONS.to_string()),
        });

        // `:256-262` — `manualAbort` is this token; `:300`'s `abort()` fires it.
        let manual_abort = CancelToken::new();
        let mut manual = Box::pin(
            interaction.prompt(
                AuthPrompt::manual_code(MANUAL_PROMPT_MESSAGE)
                    .with_placeholder(redirect_uri.clone())
                    .with_cancel(manual_abort.clone()),
            ),
        );
        let mut waiter = Box::pin(server.wait());

        enum Winner {
            Redirect(Result<Option<AuthorizationCallback>, OAuthError>),
            Manual(Result<String, OAuthError>),
        }
        // `:272` raced against `:256-270`.
        let winner = tokio::select! {
            settled = &mut waiter => Winner::Redirect(settled),
            prompted = &mut manual => Winner::Manual(prompted),
        };

        let mut manual_input: Option<String> = None;
        let redirect_result = match winner {
            Winner::Redirect(settled) => {
                // `:300` — the redirect won, so abort the pending prompt.
                manual_abort.cancel();
                settled?
            }
            Winner::Manual(prompted) => {
                // `:273` rethrows a prompt rejection before the redirect result is consulted.
                let input = prompted?;
                manual_input = Some(input);
                // `:265`/`:269`
                server.cancel_wait();
                waiter.await?
            }
        };

        // `:274-282`
        let mut code: Option<String> = None;
        let mut state: Option<String> = None;
        match redirect_result {
            Some(result) if !result.code.is_empty() => {
                code = Some(result.code);
                state = Some(result.state);
            }
            _ => {
                if let Some(input) = manual_input.as_deref() {
                    let parsed = parse_authorization_input(input);
                    // `:279`
                    if truthy(parsed.state.as_deref())
                        && parsed.state.as_deref() != Some(pkce.verifier.as_str())
                    {
                        return Err(OAuthError::Failed("OAuth state mismatch".to_string()));
                    }
                    code = parsed.code;
                    // `:281` — `??`, so an empty-string state is kept rather than defaulted.
                    state = parsed.state.or_else(|| Some(pkce.verifier.clone()));
                }
            }
        }

        // `:284-293` — the second chance. The wait only settles early via `cancel_wait`, so this
        // fires only when the listener stopped on its own.
        if !truthy(code.as_deref()) && manual_input.is_none() {
            let input = manual.await?;
            let parsed = parse_authorization_input(&input);
            if truthy(parsed.state.as_deref())
                && parsed.state.as_deref() != Some(pkce.verifier.as_str())
            {
                return Err(OAuthError::Failed("OAuth state mismatch".to_string()));
            }
            code = parsed.code;
            state = parsed.state.or_else(|| Some(pkce.verifier.clone()));
        }

        // `:295-296`
        if !truthy(code.as_deref()) {
            return Err(OAuthError::Failed("Missing authorization code".to_string()));
        }
        if !truthy(state.as_deref()) {
            return Err(OAuthError::Failed("Missing OAuth state".to_string()));
        }
        let code = code.unwrap_or_default();
        let state = state.unwrap_or_default();

        // `:297`
        interaction.notify(AuthEvent::Progress {
            message: EXCHANGE_PROGRESS_MESSAGE.to_string(),
        });
        // `:298`
        self.exchange_authorization_code(&code, &state, &pkce.verifier, &redirect_uri)
            .await
    }
}

#[async_trait::async_trait]
impl OAuthAuth for AnthropicOAuth {
    /// `anthropic.ts:343`
    fn name(&self) -> &str {
        "Anthropic (Claude Pro/Max)"
    }

    /// `isSubscription: true` (pi v0.84.1 `oauth/anthropic.ts:357`) — a Claude Pro/Max plan, not
    /// metered API billing.
    fn is_subscription(&self) -> bool {
        true
    }

    /// 1:1 port of `loginAnthropic` (`anthropic.ts:229-303`, wired at `:344`): start the callback
    /// listener, show the authorize URL, race the browser redirect against a manual paste, then
    /// exchange the code.
    ///
    /// The `select!` inside is upstream's race-by-latch: whichever of the two settles first
    /// cancels the other. When the paste wins we still consult the listener, because upstream
    /// gives a redirect that landed concurrently precedence over the pasted value (`:272-282`).
    ///
    /// This overrides the trait's `LoginUnsupported` default, so a `dyn OAuthAuth` obtained from
    /// [`super::load::load_anthropic_oauth`] runs the real flow.
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        self.run_login(interaction).await
    }

    /// `anthropic.ts:345` — `refresh: (credential) => refreshAnthropicToken(credential.refresh)`.
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        let refresh = match cred {
            Credential::Oauth { refresh, .. } => refresh.as_str(),
            Credential::ApiKey { .. } => {
                return Err(OAuthError::Failed(
                    "Anthropic OAuth refresh requires an oauth credential".to_string(),
                )
                .into_auth_error("anthropic"));
            }
        };
        self.refresh_token(refresh)
            .await
            .map_err(|e| e.into_auth_error("anthropic"))
    }

    /// `anthropic.ts:347-349` — `{ apiKey: credential.access }`.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        match cred {
            Credential::Oauth { access, .. } => Ok(ModelAuth {
                api_key: Some(access.clone()),
                ..Default::default()
            }),
            Credential::ApiKey { .. } => Err(OAuthError::Failed(
                "Anthropic OAuth toAuth requires an oauth credential".to_string(),
            )
            .into_auth_error("anthropic")),
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
    use base64::Engine as _;
    use std::io::{Read as _, Write as _};
    use std::sync::{Arc, Mutex};

    // -- upstream-derived fixtures ------------------------------------------

    /// `anthropic.ts:239-251` as `URLSearchParams.toString()` renders it: spaces in `scope` become
    /// `+`, every `:` becomes `%3A`, and `redirect_uri`'s `://` / `/` become `%3A%2F%2F` / `%2F`.
    const EXPECTED_QUERY: &str = concat!(
        "code=true",
        "&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e",
        "&response_type=code",
        "&redirect_uri=http%3A%2F%2Flocalhost%3A53692%2Fcallback",
        "&scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference",
        "+user%3Asessions%3Aclaude_code+user%3Amcp_servers+user%3Afile_upload",
        "&code_challenge=CHAL",
        "&code_challenge_method=S256",
        "&state=VERIF",
    );

    #[test]
    fn client_id_matches_upstream_base64() {
        // anthropic.ts:28-29 — `decode(...)` is `atob`.
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(CLIENT_ID_BASE64)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), CLIENT_ID);
    }

    #[test]
    fn authorization_url_matches_url_search_params() {
        let oauth = AnthropicOAuth::new();
        let url = oauth.authorization_url("CHAL", "VERIF", "http://localhost:53692/callback");
        assert_eq!(url, format!("{AUTHORIZE_URL}?{EXPECTED_QUERY}"));
    }

    #[test]
    fn constants_are_upstream_verbatim() {
        // anthropic.ts:30-37
        assert_eq!(AUTHORIZE_URL, "https://claude.ai/oauth/authorize");
        assert_eq!(TOKEN_URL, "https://platform.claude.com/v1/oauth/token");
        assert_eq!(CALLBACK_PORT, 53692);
        assert_eq!(CALLBACK_PATH, "/callback");
        assert_eq!(
            SCOPES,
            "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
        );
    }

    // -- parseAuthorizationInput, anthropic.ts:52-79 -------------------------

    fn parsed(code: Option<&str>, state: Option<&str>) -> ParsedAuthorizationInput {
        ParsedAuthorizationInput {
            code: code.map(str::to_string),
            state: state.map(str::to_string),
        }
    }

    #[test]
    fn parses_full_redirect_url() {
        assert_eq!(
            parse_authorization_input(
                "  http://localhost:53692/callback?code=abc123&state=xyz789  "
            ),
            parsed(Some("abc123"), Some("xyz789"))
        );
    }

    #[test]
    fn parses_url_with_percent_encoding() {
        assert_eq!(
            parse_authorization_input("https://claude.ai/cb?code=a%2Bb&state=c%3Ad"),
            parsed(Some("a+b"), Some("c:d"))
        );
    }

    #[test]
    fn url_without_params_yields_neither_field() {
        // `:56-63` takes the URL branch even when both lookups miss.
        assert_eq!(
            parse_authorization_input("http://localhost:53692/callback"),
            parsed(None, None)
        );
    }

    #[test]
    fn url_fragment_is_not_searchable() {
        assert_eq!(
            parse_authorization_input("http://localhost/cb#code=nope"),
            parsed(None, None)
        );
    }

    #[test]
    fn parses_hash_form() {
        assert_eq!(
            parse_authorization_input("thecode#thestate"),
            parsed(Some("thecode"), Some("thestate"))
        );
    }

    #[test]
    fn hash_form_truncates_like_js_split_limit_two() {
        // JS `"a#b#c".split("#", 2)` === ["a", "b"] — the remainder is DROPPED, not rejoined.
        assert_eq!(
            parse_authorization_input("a#b#c"),
            parsed(Some("a"), Some("b"))
        );
    }

    #[test]
    fn parses_bare_query_string() {
        assert_eq!(
            parse_authorization_input("code=abc&state=xyz"),
            parsed(Some("abc"), Some("xyz"))
        );
    }

    #[test]
    fn empty_code_param_survives_as_empty_string() {
        // `searchParams.get("code")` is "" for `?code=`, which is falsy at the caller's checks.
        let out = parse_authorization_input("code=&state=xyz");
        assert_eq!(out.code.as_deref(), Some(""));
        assert!(!truthy(out.code.as_deref()));
    }

    #[test]
    fn parses_bare_code() {
        assert_eq!(
            parse_authorization_input("just-a-code"),
            parsed(Some("just-a-code"), None)
        );
    }

    #[test]
    fn empty_input_yields_default() {
        assert_eq!(
            parse_authorization_input("   "),
            ParsedAuthorizationInput::default()
        );
    }

    // -- error text ----------------------------------------------------------

    #[test]
    fn post_error_status_message_is_upstream_verbatim() {
        // anthropic.ts:184
        let e = PostError::Status {
            status: 400,
            url: "https://token.example/v1".to_string(),
            body: "{\"error\":\"bad\"}".to_string(),
        };
        assert_eq!(
            e.to_string(),
            "HTTP request failed. status=400; url=https://token.example/v1; body={\"error\":\"bad\"}"
        );
        assert_eq!(
            format_error_details(&e),
            "Error: HTTP request failed. status=400; url=https://token.example/v1; body={\"error\":\"bad\"}"
        );
    }

    // -- token endpoint on loopback -----------------------------------------

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
            let sink = requests.clone();
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
                url: format!("http://127.0.0.1:{port}/v1/oauth/token"),
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

    fn strategy_for(url: &str) -> AnthropicOAuth {
        AnthropicOAuth::with_endpoints(AUTHORIZE_URL, url, "127.0.0.1", 0)
    }

    #[tokio::test]
    async fn exchange_sends_upstream_request_shape_and_computes_expiry() {
        let mut server = FakeTokenServer::start(
            200,
            r#"{"access_token":"acc","refresh_token":"ref","expires_in":3600}"#,
        );
        let oauth = strategy_for(&server.url);
        let before = now_ms();
        let cred = oauth
            .exchange_authorization_code(
                "CODE",
                "STATE",
                "VERIF",
                "http://localhost:53692/callback",
            )
            .await
            .unwrap();
        let after = now_ms();

        let (head, body) = server.recorded();
        assert!(head.starts_with("POST /v1/oauth/token HTTP/1.1"), "{head}");
        assert!(
            head.to_lowercase()
                .contains("content-type: application/json"),
            "{head}"
        );
        assert!(
            head.to_lowercase().contains("accept: application/json"),
            "{head}"
        );
        // anthropic.ts:198-205
        assert_eq!(
            body,
            serde_json::json!({
                "grant_type": "authorization_code",
                "client_id": CLIENT_ID,
                "code": "CODE",
                "state": "STATE",
                "redirect_uri": "http://localhost:53692/callback",
                "code_verifier": "VERIF",
            })
        );

        match cred {
            Credential::Oauth {
                access,
                refresh,
                expires,
                ..
            } => {
                assert_eq!(access, "acc");
                assert_eq!(refresh, "ref");
                // anthropic.ts:225 — Date.now() + expires_in*1000 - 5*60*1000, in MILLISECONDS.
                // The literals are upstream's, deliberately NOT `EXPIRY_SKEW_MS`: asserting
                // against the constant under test would make the assertion self-fulfilling.
                assert!(expires >= before + 3_600_000 - 300_000, "{expires}");
                assert!(expires <= after + 3_600_000 - 300_000, "{expires}");
            }
            other => panic!("expected oauth credential, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn exchange_wraps_http_failure_with_upstream_message() {
        let server = FakeTokenServer::start(400, "nope");
        let oauth = strategy_for(&server.url);
        let err = oauth
            .exchange_authorization_code(
                "CODE",
                "STATE",
                "VERIF",
                "http://localhost:53692/callback",
            )
            .await
            .unwrap_err();
        // anthropic.ts:206-210, wrapping :184.
        assert_eq!(
            err.to_string(),
            format!(
                "Token exchange request failed. url={}; redirect_uri=http://localhost:53692/callback; response_type=authorization_code; details=Error: HTTP request failed. status=400; url={}; body=nope",
                server.url, server.url
            )
        );
    }

    #[tokio::test]
    async fn exchange_wraps_invalid_json_with_upstream_message() {
        let server = FakeTokenServer::start(200, "not json");
        let oauth = strategy_for(&server.url);
        let err = oauth
            .exchange_authorization_code(
                "CODE",
                "STATE",
                "VERIF",
                "http://localhost:53692/callback",
            )
            .await
            .unwrap_err();
        // anthropic.ts:215-219
        assert!(
            err.to_string().starts_with(&format!(
                "Token exchange returned invalid JSON. url={}; body=not json; details=Error: ",
                server.url
            )),
            "{err}"
        );
    }

    #[tokio::test]
    async fn refresh_sends_upstream_request_shape() {
        let mut server = FakeTokenServer::start(
            200,
            r#"{"access_token":"a2","refresh_token":"r2","expires_in":60,"scope":"user:inference"}"#,
        );
        let oauth = strategy_for(&server.url);
        let cred = oauth
            .refresh(&oauth_credential("old-access", "old-refresh", 0))
            .await
            .unwrap();

        let (_, body) = server.recorded();
        // anthropic.ts:311-315 — no redirect_uri, no code_verifier.
        assert_eq!(
            body,
            serde_json::json!({
                "grant_type": "refresh_token",
                "client_id": CLIENT_ID,
                "refresh_token": "old-refresh",
            })
        );
        match cred {
            Credential::Oauth {
                access, refresh, ..
            } => {
                assert_eq!(access, "a2");
                // Upstream stores the ROTATED refresh token, not the one it sent (`:337`).
                assert_eq!(refresh, "r2");
            }
            other => panic!("expected oauth credential, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refresh_wraps_http_failure_with_upstream_message() {
        let server = FakeTokenServer::start(401, "invalid_grant");
        let oauth = strategy_for(&server.url);
        let err = oauth.refresh_token("dead").await.unwrap_err();
        // anthropic.ts:316-318
        assert_eq!(
            err.to_string(),
            format!(
                "Anthropic token refresh request failed. url={}; details=Error: HTTP request failed. status=401; url={}; body=invalid_grant",
                server.url, server.url
            )
        );
    }

    #[tokio::test]
    async fn refresh_wraps_invalid_json_with_upstream_message() {
        let server = FakeTokenServer::start(200, "<html/>");
        let oauth = strategy_for(&server.url);
        let err = oauth.refresh_token("tok").await.unwrap_err();
        // anthropic.ts:328-332
        assert!(
            err.to_string().starts_with(&format!(
                "Anthropic token refresh returned invalid JSON. url={}; body=<html/>; details=Error: ",
                server.url
            )),
            "{err}"
        );
    }

    #[tokio::test]
    async fn refresh_failure_folds_into_the_oauth_taxonomy() {
        let server = FakeTokenServer::start(401, "invalid_grant");
        let oauth = strategy_for(&server.url);
        let err = oauth
            .refresh(&oauth_credential("a", "dead", 0))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "oauth");
        assert!(
            std::error::Error::source(&err)
                .unwrap()
                .to_string()
                .starts_with("Anthropic token refresh request failed."),
            "{err}"
        );
    }

    #[tokio::test]
    async fn to_auth_exposes_access_token_as_api_key() {
        // anthropic.ts:347-349
        let oauth = AnthropicOAuth::new();
        let auth = oauth
            .to_auth(&oauth_credential("the-access-token", "r", 0))
            .await
            .unwrap();
        assert_eq!(auth.api_key.as_deref(), Some("the-access-token"));
        assert!(auth.headers.is_none());
        assert!(auth.base_url.is_none());
    }

    #[test]
    fn strategy_name_is_upstream_display_name() {
        // anthropic.ts:343
        assert_eq!(AnthropicOAuth::new().name(), "Anthropic (Claude Pro/Max)");
        // `anthropicOAuth` (`:342-350`) declares no `loginLabel`.
        assert!(AnthropicOAuth::new().login_label().is_none());
    }

    /// `login` must be the **trait** member, not an inherent method: `load_anthropic_oauth()`
    /// hands out an `Arc<dyn OAuthAuth>`, and an inherent `login` would be shadowed by the
    /// trait's `LoginUnsupported` default, turning `cyrup login anthropic` into an error.
    #[tokio::test]
    async fn login_dispatches_through_dyn_oauth_auth() {
        let mut token = FakeTokenServer::start(
            200,
            r#"{"access_token":"dyn-access","refresh_token":"dyn-refresh","expires_in":3600}"#,
        );
        let flow: Arc<dyn OAuthAuth> = Arc::new(strategy_for(&token.url));
        let interaction = ScriptedInteraction::new(vec![Ok("DYNCODE".to_string())]);
        let cred = flow.login(&interaction).await.unwrap();
        match cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "dyn-access"),
            other => panic!("expected oauth credential, got {other:?}"),
        }
        let (_, body) = token.recorded();
        assert_eq!(body.get("code").and_then(|v| v.as_str()), Some("DYNCODE"));
    }

    // -- login, anthropic.ts:229-303 -----------------------------------------

    /// Issue a bare HTTP/1.1 GET against a loopback callback listener.
    fn http_get(port: u16, target: &str) -> (String, String) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let req = format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
        (
            head.lines().next().unwrap_or("").to_string(),
            body.to_string(),
        )
    }

    fn auth_url_of(interaction: &ScriptedInteraction) -> String {
        for event in interaction.events() {
            if let AuthEvent::AuthUrl { url, .. } = event {
                return url;
            }
        }
        panic!("no auth_url event was emitted")
    }

    fn param_of(url: &str, key: &str) -> String {
        let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
        query_get(query, key).unwrap_or_default()
    }

    /// Poll until the flow has emitted its authorize URL, then return `(port, state)`.
    async fn await_auth_url(interaction: &ScriptedInteraction) -> (u16, String) {
        for _ in 0..2_000 {
            if !interaction.events().is_empty() {
                let url = auth_url_of(interaction);
                let redirect = param_of(&url, "redirect_uri");
                let port = redirect
                    .rsplit_once(':')
                    .and_then(|(_, rest)| rest.split('/').next())
                    .and_then(|p| p.parse().ok())
                    .unwrap();
                return (port, param_of(&url, "state"));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the login never emitted an auth_url event")
    }

    /// The whole flow on the browser-redirect path.
    #[tokio::test]
    async fn login_completes_via_browser_redirect() {
        let mut token = FakeTokenServer::start(
            200,
            r#"{"access_token":"live-access","refresh_token":"live-refresh","expires_in":3600}"#,
        );
        let oauth = strategy_for(&token.url);
        // The manual prompt never answers, so only the redirect can complete the login.
        let interaction = Arc::new(ScriptedInteraction::new(Vec::new()).blocking_when_empty());

        let driver = {
            let interaction = interaction.clone();
            tokio::spawn(async move {
                let (port, state) = await_auth_url(&interaction).await;
                tokio::task::spawn_blocking(move || {
                    http_get(port, &format!("/callback?code=REDIRECT_CODE&state={state}"))
                })
                .await
                .unwrap()
            })
        };

        let cred = oauth.login(interaction.as_ref()).await.unwrap();
        let (status, page) = driver.await.unwrap();
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
        // anthropic.ts:145-146
        assert!(
            page.contains("Anthropic authentication completed. You can close this window."),
            "{page}"
        );

        match cred {
            Credential::Oauth {
                access, refresh, ..
            } => {
                assert_eq!(access, "live-access");
                assert_eq!(refresh, "live-refresh");
            }
            other => panic!("expected oauth credential, got {other:?}"),
        }

        let (_, body) = token.recorded();
        assert_eq!(
            body.get("code").and_then(|v| v.as_str()),
            Some("REDIRECT_CODE")
        );
        assert_eq!(
            body.get("grant_type").and_then(|v| v.as_str()),
            Some("authorization_code")
        );
        // The redirect's `state` is the verifier, and the verifier is the `code_verifier` (`:298`).
        let url = auth_url_of(&interaction);
        assert_eq!(
            body.get("code_verifier").and_then(|v| v.as_str()),
            Some(param_of(&url, "state").as_str())
        );

        // anthropic.ts:249-254 then :297 — the two notifications, in order.
        let events = interaction.events();
        assert!(matches!(events.first(), Some(AuthEvent::AuthUrl { .. })));
        assert_eq!(
            events.get(1),
            Some(&AuthEvent::Progress {
                message: "Exchanging authorization code for tokens...".to_string()
            })
        );
        // anthropic.ts:256-261 — the manual prompt is offered with the redirect URI as its
        // placeholder.
        let prompts = interaction.prompts();
        let prompt = prompts.first().unwrap();
        assert_eq!(prompt.message, MANUAL_PROMPT_MESSAGE);
        assert_eq!(
            prompt.placeholder.as_deref(),
            Some(param_of(&url, "redirect_uri").as_str())
        );
    }

    /// MIRROR of the redirect path on the manual-paste channel — same outcome, different input.
    #[tokio::test]
    async fn login_completes_via_manual_paste() {
        let mut token = FakeTokenServer::start(
            200,
            r#"{"access_token":"pasted-access","refresh_token":"pasted-refresh","expires_in":3600}"#,
        );
        let oauth = strategy_for(&token.url);
        // A bare code with no state: upstream falls back to the verifier (`:281`).
        let interaction = ScriptedInteraction::new(vec![Ok("PASTED_CODE".to_string())]);

        let cred = oauth.login(&interaction).await.unwrap();
        match cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "pasted-access"),
            other => panic!("expected oauth credential, got {other:?}"),
        }

        let (_, body) = token.recorded();
        assert_eq!(
            body.get("code").and_then(|v| v.as_str()),
            Some("PASTED_CODE")
        );
        let state = body.get("state").and_then(|v| v.as_str()).unwrap();
        let verifier = body.get("code_verifier").and_then(|v| v.as_str()).unwrap();
        assert_eq!(state, verifier, "`:281` defaults state to the verifier");
        assert_eq!(state, param_of(&auth_url_of(&interaction), "state"));
    }

    /// Answers the manual prompt with a full redirect URL carrying the flow's own state, read back
    /// out of the `auth_url` event this interaction just received.
    #[derive(Default)]
    struct PasteRedirectUrl {
        state: Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl AuthInteraction for PasteRedirectUrl {
        async fn prompt(&self, _prompt: AuthPrompt) -> Result<String, OAuthError> {
            let state = self.state.lock().ok().and_then(|s| s.clone());
            match state {
                Some(state) => Ok(format!(
                    "http://localhost:53692/callback?code=URLCODE&state={state}"
                )),
                None => Err(OAuthError::Failed("no auth_url seen".to_string())),
            }
        }
        fn notify(&self, event: AuthEvent) {
            if let AuthEvent::AuthUrl { url, .. } = event
                && let Ok(mut slot) = self.state.lock()
            {
                *slot = Some(param_of(&url, "state"));
            }
        }
    }

    #[tokio::test]
    async fn login_accepts_a_pasted_redirect_url_with_the_matching_state() {
        let mut token = FakeTokenServer::start(
            200,
            r#"{"access_token":"url-access","refresh_token":"url-refresh","expires_in":3600}"#,
        );
        let oauth = strategy_for(&token.url);
        let interaction = PasteRedirectUrl::default();
        let cred = oauth.login(&interaction).await.unwrap();
        match cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "url-access"),
            other => panic!("expected oauth credential, got {other:?}"),
        }
        let (_, body) = token.recorded();
        assert_eq!(body.get("code").and_then(|v| v.as_str()), Some("URLCODE"));
    }

    #[tokio::test]
    async fn login_rejects_pasted_state_mismatch() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![Ok(
            "http://localhost:53692/callback?code=C&state=not-the-verifier".to_string(),
        )]);
        let err = oauth.login(&interaction).await.unwrap_err();
        // anthropic.ts:279
        assert_eq!(err.to_string(), "OAuth state mismatch");
    }

    #[tokio::test]
    async fn login_rejects_empty_paste_as_missing_code() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![Ok("   ".to_string())]);
        let err = oauth.login(&interaction).await.unwrap_err();
        // anthropic.ts:295
        assert_eq!(err.to_string(), "Missing authorization code");
    }

    #[tokio::test]
    async fn login_rejects_paste_with_empty_state_as_missing_state() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        // `?code=C&state=` — `state` is "" so the mismatch check is skipped (`:279` is falsy) and
        // `??` keeps "" rather than defaulting, so `:296` fires.
        let interaction = ScriptedInteraction::new(vec![Ok(
            "http://localhost:53692/callback?code=C&state=".to_string(),
        )]);
        let err = oauth.login(&interaction).await.unwrap_err();
        assert_eq!(err.to_string(), "Missing OAuth state");
    }

    #[tokio::test]
    async fn login_propagates_prompt_cancellation() {
        // anthropic.ts:267-270 + :273 — a rejected prompt aborts the whole login.
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![Err(OAuthError::Cancelled)]);
        let err = oauth.login(&interaction).await.unwrap_err();
        assert_eq!(err.to_string(), "Login cancelled");
    }

    /// The handler's rejection branches answer the browser and keep listening, so a subsequent,
    /// correct redirect still completes the login — `:127-141` never reach `settleWait`.
    #[tokio::test]
    async fn bad_redirects_are_answered_without_ending_the_login() {
        let mut token = FakeTokenServer::start(
            200,
            r#"{"access_token":"second-try","refresh_token":"r","expires_in":3600}"#,
        );
        let oauth = strategy_for(&token.url);
        let interaction = Arc::new(ScriptedInteraction::new(Vec::new()).blocking_when_empty());

        let driver = {
            let interaction = interaction.clone();
            tokio::spawn(async move {
                let (port, state) = await_auth_url(&interaction).await;
                tokio::task::spawn_blocking(move || {
                    // anthropic.ts:126-130
                    let denied = http_get(port, "/callback?error=access_denied");
                    // anthropic.ts:133-136
                    let missing = http_get(port, "/callback?code=only");
                    // anthropic.ts:138-141
                    let mismatch = http_get(port, "/callback?code=C&state=wrong");
                    // …and then the good one.
                    let ok = http_get(port, &format!("/callback?code=GOOD&state={state}"));
                    (denied, missing, mismatch, ok)
                })
                .await
                .unwrap()
            })
        };

        let cred = oauth.login(interaction.as_ref()).await.unwrap();
        let (denied, missing, mismatch, ok) = driver.await.unwrap();

        assert!(denied.0.starts_with("HTTP/1.1 400"), "{denied:?}");
        assert!(
            denied
                .1
                .contains("Anthropic authentication did not complete."),
            "{denied:?}"
        );
        assert!(denied.1.contains("Error: access_denied"), "{denied:?}");

        assert!(missing.0.starts_with("HTTP/1.1 400"), "{missing:?}");
        assert!(
            missing.1.contains("Missing code or state parameter."),
            "{missing:?}"
        );

        assert!(mismatch.0.starts_with("HTTP/1.1 400"), "{mismatch:?}");
        assert!(mismatch.1.contains("State mismatch."), "{mismatch:?}");

        assert!(ok.0.starts_with("HTTP/1.1 200"), "{ok:?}");
        match cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "second-try"),
            other => panic!("expected oauth credential, got {other:?}"),
        }
        let (_, body) = token.recorded();
        assert_eq!(body.get("code").and_then(|v| v.as_str()), Some("GOOD"));
    }
}
