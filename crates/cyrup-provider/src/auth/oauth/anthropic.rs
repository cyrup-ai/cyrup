//! Anthropic OAuth flow (Claude Pro/Max) — 1:1 port of pi
//! `packages/ai/src/auth/oauth/anthropic.ts`, first from v0.83.0 (350 lines) and re-based onto
//! v1.1.0 (307 lines) where upstream changed behaviour since: the callback handler, now upstream's
//! shared `callback-server.ts` (PROV-137); the degraded and free-port listener starts (PROV-117,
//! PROV-136); the pasted-state handling (PROV-135); and the method selector and copy-code login
//! (PROV-120). Every unmarked `:N` cite in this file is v0.83.0 `anthropic.ts`; a cite into a later
//! version carries its `@v1.1.0` (or `@<commit>`) marker, and one into another file names it (or
//! follows, in the same sentence, a cite that does).
//!
//! This is the subscription login: a PKCE authorization-code flow whose redirect lands on the
//! loopback callback listener, raced against a manual-paste prompt for the case where the browser
//! runs on another machine. PROV-120 adds upstream's method selector in front of it and a second,
//! copy-code login that never starts a listener at all (the headless path). It also owns the
//! refresh-token exchange that [`crate::auth::resolve`] drives under the credential-store lock.
//!
//! ## Provenance
//!
//! | this module | `anthropic.ts` |
//! |---|---|
//! | [`CLIENT_ID`] / [`AUTHORIZE_URL`] / [`TOKEN_URL`] / [`SCOPES`] / [`CALLBACK_PORT`] | `:29-37` |
//! | [`parse_authorization_input`] | `parseAuthorizationInput`, `:52-79` |
//! | [`format_error_details`] | `formatErrorDetails`, `:81-97` |
//! | [`AnthropicCallbackHandler`] | the shared `startOAuthCallbackServer` request handler, `callback-server.ts:78-116` @v1.1.0 (PROV-137; was `:114-148`) |
//! | [`post_json`] | `postJson`, `:170-188` |
//! | [`AnthropicOAuth::exchange_authorization_code`] | `exchangeAuthorizationCode`, `:190-227` |
//! | [`AnthropicOAuth::login`] | `loginAnthropic`, `:229-303`; the method selector, `:282-299` @v1.1.0; the free-port fallback, `:140-198` @`8d8ae2fc2` |
//! | [`AnthropicOAuth::login_copy_code`] | `loginAnthropicCopyCode`, `:200-235` @v1.1.0 (PROV-120) |
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
//!   never a browser. The 404-on-foreign-route branch (`callback-server.ts:81-84` @v1.1.0) lives
//!   in that shared server. Two consequences, neither reachable by a browser following a
//!   redirect, both shared with every flow on that server ([`super::openai_codex`],
//!   [`super::radius`]): the 404 page reads `"OAuth callback route not found."` where upstream
//!   says `"Callback route not found."`; and the `claimed || settled` 409 (`:89-92` @v1.1.0) is
//!   checked by the shared server before the handler's state check (`:85-88` @v1.1.0) and reads
//!   `"This OAuth callback has already been used."` where upstream says `"This sign-in has already
//!   been handled."` — after the first good redirect the server stops accepting, so only a request
//!   racing that one could see either page.
//! * **Cancellation.** Upstream threads an `AbortSignal` into the manual prompt and calls
//!   `manualAbort.abort()` in `finally` (`:232`, `:261`, `:300`; `callback-server.ts:160-182`
//!   @v1.1.0); here that signal is a [`CancelToken`] on `AuthPrompt::cancel`, fired by a drop guard
//!   on every exit from the wait — redirect won, paste won, either failed — as upstream's
//!   `finally`. `server.server.close()` (`:301`) is the [`super::callback::CallbackServer`] drop.
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
use super::interaction::{AuthEvent, AuthInteraction, AuthPrompt, AuthSelectOption};
use super::pkce::generate_pkce;
use super::query::{encode_query, parse_query};
use super::{OAuthError, now_ms, oauth_credential};
use crate::auth::types::{Credential, EnvAuthContext, ModelAuth, ProviderEnv};
use crate::auth::{LoginOptions, OAuthAuth};
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
/// `anthropic.ts:18-20` @v1.1.0+ — the *preferred* port, so it can be forwarded into a container
/// or over SSH. Anthropic accepts any loopback port, so since `8d8ae2fc2` (pi #10571) a login
/// that cannot bind it (a concurrent login, a reserved range) falls back to an OS-chosen free
/// port; see [`AnthropicOAuth::login`].
pub const CALLBACK_PORT: u16 = 53692;
/// `anthropic.ts:34`.
pub const CALLBACK_PATH: &str = "/callback";
/// `anthropic.ts:35` — the redirect URI is advertised on `localhost` even though the listener
/// binds `CALLBACK_HOST` (`127.0.0.1` unless `*_OAUTH_CALLBACK_HOST` says otherwise).
pub const ADVERTISE_HOST: &str = "localhost";

/// `REDIRECT_URI` (`anthropic.ts:22` @v1.1.0+) — upstream composes it from the port and path,
/// ``http://localhost:${CALLBACK_PORT}${CALLBACK_PATH}``. Since `8d8ae2fc2` it is only the
/// fallback of `callback?.redirectUri ?? REDIRECT_URI` (`:157` @v1.1.0): the browser flow
/// advertises and exchanges against whatever its listener bound, and this constant is used only
/// when no listener came up at all (PROV-117), so the paste still names the preferred redirect.
pub const REDIRECT_URI: &str = "http://localhost:53692/callback";
/// `COPY_CODE_REDIRECT_URI` (`anthropic.ts:23` @v1.1.0, `:21` @v1.0.0) — PROV-120. The
/// copy-code (headless) login registers Anthropic's own hosted callback page instead of the
/// loopback listener: the page shows `code#state` for the user to paste back, so nothing has to
/// listen on this machine and the login works over SSH, in a container, or anywhere the browser
/// cannot reach `localhost`.
pub const COPY_CODE_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
/// `ANTHROPIC_BROWSER_LOGIN_METHOD` (`anthropic.ts:24` @v1.1.0) — the select option id for the
/// loopback-redirect login.
const BROWSER_LOGIN_METHOD: &str = "browser";
/// `ANTHROPIC_COPY_CODE_LOGIN_METHOD` (`anthropic.ts:25` @v1.1.0) — the select option id for the
/// copy-code login.
const COPY_CODE_LOGIN_METHOD: &str = "copy_code";
/// `anthropic.ts:36-37`. Space-separated; the urlencoded serializer turns the spaces into `+`.
pub const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

/// `anthropic.ts:252-253`.
const AUTH_URL_INSTRUCTIONS: &str = "Complete login in your browser. If the browser is on another machine, paste the final redirect URL here.";
/// `anthropic.ts:259`.
const MANUAL_PROMPT_MESSAGE: &str =
    "Complete login in your browser, or paste the authorization code / redirect URL here:";
/// `anthropic.ts:285` @v1.1.0.
const SELECT_LOGIN_METHOD_MESSAGE: &str = "Select Anthropic login method:";
/// `anthropic.ts:287` @v1.1.0.
const BROWSER_LOGIN_LABEL: &str = "Browser login (default)";
/// `anthropic.ts:288` @v1.1.0.
const COPY_CODE_LOGIN_LABEL: &str = "Copy code login (headless)";
/// `anthropic.ts:215` @v1.1.0 — the copy-code flow's `auth_url` instructions.
const COPY_CODE_AUTH_URL_INSTRUCTIONS: &str =
    "Complete login in your browser, then copy the code Anthropic shows and paste it here.";
/// `anthropic.ts:220` @v1.1.0 — the copy-code flow's `manual_code` prompt.
const COPY_CODE_PROMPT_MESSAGE: &str = "Paste the code Anthropic shows after you sign in:";
/// `anthropic.ts:221` @v1.1.0 — what Anthropic's hosted callback page displays.
const COPY_CODE_PLACEHOLDER: &str = "code#state";
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
// The callback handler — callback-server.ts:78-116 @v1.1.0 (PROV-137)
// ---------------------------------------------------------------------------

/// A `code`/`state` pair delivered by the browser redirect (`anthropic.ts:18`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationCallback {
    pub code: String,
    pub state: String,
}

/// The Anthropic-specific half of the callback server: the request handler of upstream's shared
/// `startOAuthCallbackServer` (`callback-server.ts:78-116` @v1.1.0), which `loginAnthropic`
/// starts with `providerName: "Anthropic"`, `state: verifier` and `complete: async (code) => code`
/// (`anthropic.ts:142-151` @v1.1.0).
///
/// PROV-137 — pi `4df157433` (v0.99.0) replaced the hand-rolled v0.83.0 handler
/// (`anthropic.ts:114-148` @v0.83.0) with that shared one, and the two differ where it matters:
/// a state-matching `?error=` redirect (the user clicked *Deny*) now **fails the login**
/// (`finish({ error })`, `callback-server.ts:93-98` @v1.1.0) instead of answering the browser and
/// leaving the login waiting on the paste prompt. A wrong or missing state, or a missing code,
/// still replies without settling, so a later good redirect can complete the login.
pub struct AnthropicCallbackHandler {
    /// `options.state` (`callback-server.ts:85` @v1.1.0), which is the PKCE verifier
    /// (`anthropic.ts:149` @v1.1.0).
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
        // `callback-server.ts:81-84` @v1.1.0 — the pathname half is checked by the shared server;
        // this is the method half.
        if request.method != "GET" {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(404, "OAuth callback route not found.", None)
                    .no_store(),
            };
        }

        // `callback-server.ts:85-88` @v1.1.0 — first, and an exact comparison, so a missing state
        // is a mismatch too.
        let state = request.param("state");
        if state != Some(self.expected_state.as_str()) {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(400, "State mismatch.", None).no_store(),
            };
        }

        // `callback-server.ts:93-99` @v1.1.0 — JS truthiness, so a bare `?error=` is not an error;
        // `??`, so a present-but-empty `error_description` is kept.
        let error = request.param("error");
        if truthy(error) {
            let error = error.unwrap_or_default();
            let description = request.param("error_description").unwrap_or(error);
            return CallbackOutcome::Failed {
                reply: CallbackReply::error(
                    400,
                    "Anthropic authorization failed.",
                    Some(description),
                )
                .no_store(),
                error: OAuthError::Failed(format!("Anthropic authorization failed: {description}")),
            };
        }

        // `callback-server.ts:100-104` @v1.1.0
        let code = request.param("code");
        if !truthy(code) {
            return CallbackOutcome::Continue {
                reply: CallbackReply::error(400, "Missing authorization code.", None).no_store(),
            };
        }

        // `callback-server.ts:105-109` @v1.1.0 — `complete: async (code) => code` cannot fail,
        // so the 502 branch (`:110-114` @v1.1.0) is unreachable for this flow.
        CallbackOutcome::Complete {
            reply: CallbackReply::success("Signed in to Anthropic. You may now close this page.")
                .no_store(),
            value: AuthorizationCallback {
                code: code.unwrap_or_default().to_string(),
                state: self.expected_state.clone(),
            },
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
    /// `None` resolves `CYRUP_OAUTH_CALLBACK_HOST` at login time, which
    /// is what upstream's module-level `CALLBACK_HOST` const does (`anthropic.ts:32`).
    callback_host: Option<String>,
    callback_port: u16,
    /// Provider-scoped environment overlay (Pi `options.env`) consulted when resolving whether the
    /// token/authorize request is itself routed through an HTTP proxy. `None` (the default, and
    /// every production construction) resolves purely from the ambient process environment, which
    /// is the pre-existing behavior. See [`Self::with_env`].
    env: Option<ProviderEnv>,
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
            env: None,
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
            env: None,
        }
    }

    /// Attach a provider-scoped environment overlay used when deciding whether the OAuth hop is
    /// itself proxied. The ported resolver has always let a non-empty overlay win over the ambient
    /// process env (`node_http_proxy::get_proxy_env`); this makes that reachable here, so a caller
    /// — or a test — can pin `http(s)_proxy`/`no_proxy` instead of inheriting the host's.
    #[must_use]
    pub fn with_env(mut self, env: ProviderEnv) -> Self {
        self.env = Some(env);
        self
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
        // `build_client_for_target` rather than `build_client_for` so `self.env` participates.
        // With `env: None` the two are identical — same `EnvAuthContext`, same ported resolver.
        crate::stream::sse::build_client_for_target(
            target_url,
            &crate::auth::types::EnvAuthContext,
            self.env.as_ref(),
            None,
        )
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

    /// The redirect URI advertised when no listener could be bound: upstream's module-level
    /// `REDIRECT_URI` (`anthropic.ts:22` @v1.1.0+), the right-hand side of `:157`'s `??` @v1.1.0. Composed
    /// the same way — advertise host, port, path — but from the *configured* callback port, so
    /// [`AnthropicOAuth::with_endpoints`]' test bind is described honestly; for the production
    /// port it is [`REDIRECT_URI`] verbatim.
    fn redirect_uri(&self) -> String {
        format!(
            "http://{ADVERTISE_HOST}:{}{CALLBACK_PATH}",
            self.callback_port
        )
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

        // `:231` — the verifier doubles as the OAuth state.
        //
        // PROV-117 — `4df157433` ("share OAuth callback server and sign-in page") ends this call
        // with `.catch(() => undefined)`, and `waitForCallbackOrManualInput(interaction, callback,
        // …)` takes `OAuthCallbackServer<T> | undefined` and documents the degraded path: "Without
        // a callback server only the manual prompt is used" (`callback-server.ts:150-183` @v1.1.0).
        // So a listener that cannot bind leaves the authorize URL and the paste prompt, and the
        // login still completes. `openrouter.ts:116` and `radius.ts:153` keep the hard failure
        // deliberately; only this flow and the Codex one degrade.
        //
        // `8d8ae2fc2` (pi #10571) puts a second attempt in between (`anthropic.ts:142-156`
        // @v1.1.0+): the preferred port first, so a forwarded 53692 (SSH, containers) keeps
        // working; then port `0`, an OS-chosen free loopback port, because Anthropic accepts any
        // loopback port and 53692 can be taken by a concurrent login or reserved outright (the
        // Hyper-V/WSL exclusion ranges on Windows); only when that also fails, no listener.
        // Upstream's `.catch` swallows every rejection, `Cancelled` included, and so does `.ok()`.
        let bind_host = self.bind_host().await;
        let start = |port: u16| {
            CallbackServer::start(
                CallbackServerConfig::fixed(port, CALLBACK_PATH)
                    .with_host(bind_host.clone())
                    // `redirectHost: "localhost"` (`:148` @v1.1.0).
                    .advertising(ADVERTISE_HOST)
                    .with_interaction(interaction),
                AnthropicCallbackHandler {
                    expected_state: pkce.verifier.clone(),
                },
            )
        };
        let server = match start(self.callback_port).await {
            Ok(server) => Some(server),
            Err(_) => start(0).await.ok(),
        };

        // `:157` @v1.1.0 — `callback?.redirectUri ?? REDIRECT_URI`. The BOUND redirect URI (which
        // on the fallback carries the OS-chosen port) is what the authorize URL (`:164` @v1.1.0),
        // the paste prompt's placeholder (`:179` @v1.1.0) and the token exchange (`:194` @v1.1.0)
        // all use, so the code is
        // exchanged against the very URI it was issued for. With no listener it is the
        // preferred-port URI, composed from the configured port by [`Self::redirect_uri`].
        let redirect_uri = match &server {
            Some(server) => server.redirect_uri().to_string(),
            None => self.redirect_uri(),
        };

        // `:249-254`
        interaction.notify(AuthEvent::AuthUrl {
            url: self.authorization_url(&pkce.challenge, &pkce.verifier, &redirect_uri),
            instructions: Some(AUTH_URL_INSTRUCTIONS.to_string()),
        });

        // `:256-262` — `manualAbort` is this token. Upstream aborts it in `finally` on every exit
        // from the wait (`:300`; `callback-server.ts:180-182` @v1.1.0): the redirect won, the
        // paste won (so a UI can dismiss the prompt it still shows), or either failed. The drop
        // guard is that `finally`: an early `?` return drops it, and the success path drops it
        // explicitly once the wait has settled, before the exchange — where upstream's
        // `waitForCallbackOrManualInput` returns.
        let manual_abort = CancelToken::new();
        let abort_manual_on_exit = manual_abort.clone().drop_guard();
        let mut manual = Box::pin(
            interaction.prompt(
                AuthPrompt::manual_code(MANUAL_PROMPT_MESSAGE)
                    .with_placeholder(redirect_uri.clone())
                    .with_cancel(manual_abort.clone()),
            ),
        );
        enum Winner {
            Redirect(Result<Option<AuthorizationCallback>, OAuthError>),
            Manual(Result<String, OAuthError>),
        }

        let mut manual_input: Option<String> = None;
        let redirect_result = match &server {
            Some(server) => {
                let mut waiter = Box::pin(server.wait());
                // `:272` raced against `:256-270`.
                let winner = tokio::select! {
                    settled = &mut waiter => Winner::Redirect(settled),
                    prompted = &mut manual => Winner::Manual(prompted),
                };
                match winner {
                    // The redirect won; the guard aborts the pending prompt (`:300`).
                    Winner::Redirect(settled) => settled?,
                    Winner::Manual(prompted) => {
                        // `:273` rethrows a prompt rejection before the redirect result is
                        // consulted.
                        let input = prompted?;
                        manual_input = Some(input);
                        // `:265`/`:269`
                        server.cancel_wait();
                        waiter.await?
                    }
                }
            }
            // PROV-117 — `await callback?.wait()` on an absent callback is `undefined`
            // (`callback-server.ts:174` @v1.1.0), so there is no race: the prompt is the only
            // channel and the code below reads it through the `manual_input.is_none()` second
            // chance at `:284-293`, which upstream reaches by the same `value !== undefined` test.
            None => None,
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

        // `:284-293` — the second chance (`const input = await manual`,
        // `callback-server.ts:177` @v1.1.0). Reached when the listener resolved without a code —
        // which is `cancel_wait`, a listener that stopped on its own, or (PROV-117) no listener at
        // all.
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
        // `callback-server.ts:180-182` @v1.1.0 — the wait has settled: `manualAbort.abort()`.
        drop(abort_manual_on_exit);

        // `:192` @v1.1.0+ — the only post-input check. There is no "Missing OAuth state": pi
        // removed it in `4df157433` (v0.99.0), so a pasted `code#` keeps its empty state through
        // `parsed.state ?? verifier` and is exchanged with `state: ""`, exactly as upstream sends.
        if !truthy(code.as_deref()) {
            return Err(OAuthError::Failed("Missing authorization code".to_string()));
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

    /// PROV-120 — 1:1 port of `loginAnthropicCopyCode` (`anthropic.ts:200-235` @v1.1.0, unchanged
    /// since v1.0.0's `:191-226`): the headless login. The authorize URL redirects to
    /// [`COPY_CODE_REDIRECT_URI`], Anthropic's hosted page, which shows `code#state`; the user
    /// pastes it into the one `manual_code` prompt. No callback listener is started and nothing is
    /// raced, so it works where no browser can reach this machine's loopback.
    async fn login_copy_code(
        &self,
        interaction: &dyn AuthInteraction,
    ) -> Result<Credential, OAuthError> {
        // `:201` @v1.1.0
        let pkce = generate_pkce()?;

        // `:202-216` @v1.1.0 — same authorize parameters as the browser flow, copy-code redirect.
        interaction.notify(AuthEvent::AuthUrl {
            url: self.authorization_url(&pkce.challenge, &pkce.verifier, COPY_CODE_REDIRECT_URI),
            instructions: Some(COPY_CODE_AUTH_URL_INSTRUCTIONS.to_string()),
        });

        // `:218-223` @v1.1.0 — `signal: interaction.signal` is the login-wide cancel.
        let mut prompt = AuthPrompt::manual_code(COPY_CODE_PROMPT_MESSAGE)
            .with_placeholder(COPY_CODE_PLACEHOLDER);
        prompt.cancel = interaction.cancel().cloned();
        let input = interaction.prompt(prompt).await?;

        // `:224-226` @v1.1.0 — the browser flow's checks. There is no "Missing OAuth state" check
        // here: upstream has none in either flow (removed in `4df157433`, v0.99.0), and `??` below
        // keeps an empty state — the same as `run_login`.
        let parsed = parse_authorization_input(&input);
        if truthy(parsed.state.as_deref())
            && parsed.state.as_deref() != Some(pkce.verifier.as_str())
        {
            return Err(OAuthError::Failed("OAuth state mismatch".to_string()));
        }
        let code = match parsed.code {
            Some(code) if !code.is_empty() => code,
            _ => return Err(OAuthError::Failed("Missing authorization code".to_string())),
        };

        // `:227` @v1.1.0
        interaction.notify(AuthEvent::Progress {
            message: EXCHANGE_PROGRESS_MESSAGE.to_string(),
        });
        // `:228-234` @v1.1.0 — `parsed.state ?? verifier`, exchanged against the copy-code
        // redirect.
        let state = parsed.state.unwrap_or_else(|| pkce.verifier.clone());
        self.exchange_authorization_code(&code, &state, &pkce.verifier, COPY_CODE_REDIRECT_URI)
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
    /// PROV-117 / pi `8d8ae2fc2` — a listener that cannot bind is **not** fatal:
    /// `startCallbackServer(CALLBACK_PORT).catch(() => startCallbackServer(0)).catch(() =>
    /// undefined)` (`anthropic.ts:154-156` @v1.1.0+) first falls back to an OS-chosen free port —
    /// advertised, prompted and exchanged as the bound redirect URI — and only if that also fails
    /// hands `waitForCallbackOrManualInput` an absent callback, so the paste prompt runs alone
    /// (`callback-server.ts:150-183` @v1.1.0). So `/login anthropic` still completes on a host
    /// where port 53692 is already taken, reserved, or the loopback listen is refused.
    ///
    /// PROV-120 — `anthropicOAuth.login` (`anthropic.ts:282-299` @v1.1.0) first asks which method
    /// to run: `browser` is the flow above, unchanged; `copy_code` is
    /// [`AnthropicOAuth::login_copy_code`], the headless login with no listener. The `select`
    /// answer is an option **id**, and anything else is upstream's
    /// `Unknown Anthropic login method: {method}` (`:296` @v1.1.0).
    ///
    /// This overrides the trait's `LoginUnsupported` default, so the `dyn OAuthAuth` the provider
    /// carries ([`crate::providers::builtin_oauth::builtin_provider_oauth`]) runs the real flow.
    async fn login(
        &self,
        interaction: &dyn AuthInteraction,
        _options: &LoginOptions,
    ) -> Result<Credential, OAuthError> {
        // `:283-290` @v1.1.0
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
                        id: COPY_CODE_LOGIN_METHOD.to_string(),
                        label: COPY_CODE_LOGIN_LABEL.to_string(),
                        description: None,
                    },
                ],
            ))
            .await?;

        // `:292-297` @v1.1.0
        if method == COPY_CODE_LOGIN_METHOD {
            return self.login_copy_code(interaction).await;
        }
        if method != BROWSER_LOGIN_METHOD {
            return Err(OAuthError::Failed(format!(
                "Unknown Anthropic login method: {method}"
            )));
        }
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
    use crate::auth::oauth::callback::bind_attempts;
    use crate::auth::oauth::interaction::{AuthPromptKind, ScriptedInteraction};
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
        // anthropic.ts:23-25 @v1.1.0 (PROV-120)
        assert_eq!(
            COPY_CODE_REDIRECT_URI,
            "https://platform.claude.com/oauth/code/callback"
        );
        assert_eq!(BROWSER_LOGIN_METHOD, "browser");
        assert_eq!(COPY_CODE_LOGIN_METHOD, "copy_code");
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

        /// How many requests reached the endpoint so far, without waiting for one.
        fn hits(&self) -> usize {
            self.requests.lock().unwrap().len()
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

    /// `login` must be the **trait** member, not an inherent method: `builtin_provider_oauth`
    /// hands out an `Arc<dyn OAuthAuth>`, and an inherent `login` would be shadowed by the
    /// trait's `LoginUnsupported` default, turning `cyrup login anthropic` into an error.
    #[tokio::test]
    async fn login_dispatches_through_dyn_oauth_auth() {
        let mut token = FakeTokenServer::start(
            200,
            r#"{"access_token":"dyn-access","refresh_token":"dyn-refresh","expires_in":3600}"#,
        );
        let flow: Arc<dyn OAuthAuth> = Arc::new(strategy_for(&token.url));
        let interaction = ScriptedInteraction::new(vec![
            Ok(BROWSER_LOGIN_METHOD.to_string()),
            Ok("DYNCODE".to_string()),
        ]);
        let cred = flow
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap();
        match cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "dyn-access"),
            other => panic!("expected oauth credential, got {other:?}"),
        }
        let (_, body) = token.recorded();
        assert_eq!(body.get("code").and_then(|v| v.as_str()), Some("DYNCODE"));
    }

    // -- login, anthropic.ts:229-303 -----------------------------------------

    /// The per-test deadline the redirect-driven tests put on the login and on the driver: pi's
    /// vitest default is 5s per test. Without it, a regression in which the login never settles
    /// (a missing free-port fallback advertises a squatted port whose backlog never accepts,
    /// while the manual prompt blocks forever) hangs the suite instead of failing it.
    const REDIRECT_TEST_DEADLINE: Duration = Duration::from_secs(10);

    /// Await `future`, failing the test with `what` if it outlives [`REDIRECT_TEST_DEADLINE`].
    async fn within<F: std::future::Future>(what: &str, future: F) -> F::Output {
        tokio::time::timeout(REDIRECT_TEST_DEADLINE, future)
            .await
            .unwrap_or_else(|_| panic!("{what} did not finish within {REDIRECT_TEST_DEADLINE:?}"))
    }

    /// Issue a bare HTTP/1.1 GET against a loopback callback listener. Reads time out after
    /// [`REDIRECT_TEST_DEADLINE`], so a listener that accepted nothing fails the test.
    fn http_get(port: u16, target: &str) -> (String, String) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(REDIRECT_TEST_DEADLINE))
            .unwrap();
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
        // The selector picks the browser flow (PROV-120); the manual prompt then never answers,
        // so only the redirect can complete the login.
        let interaction = Arc::new(
            ScriptedInteraction::new(vec![Ok(BROWSER_LOGIN_METHOD.to_string())])
                .blocking_when_empty(),
        );

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

        let cred = within(
            "the login",
            oauth.login(interaction.as_ref(), &LoginOptions::default()),
        )
        .await
        .unwrap();
        let (status, page) = within("the redirect", driver).await.unwrap();
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
        // `callback-server.ts:108` @v1.1.0; pi `test/anthropic-oauth.test.ts:220` asserts
        // `toContain("Signed in to Anthropic.")`.
        assert!(
            page.contains("Signed in to Anthropic. You may now close this page."),
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
        // pi `8d8ae2fc2` — `expect(login.exchangedRedirectUri).toBe(login.redirectUri)`: the code
        // is exchanged against the bound redirect URI the authorize URL advertised (`:164`, `:194`
        // @v1.1.0).
        assert_eq!(
            body.get("redirect_uri").and_then(|v| v.as_str()),
            Some(param_of(&url, "redirect_uri").as_str())
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
        // anthropic.ts:256-261 — after the selector, the manual prompt is offered with the
        // redirect URI as its placeholder.
        let prompts = interaction.prompts();
        assert_eq!(prompts[0].kind, Some(AuthPromptKind::Select));
        // PROV-120 — choosing `browser` keeps the loopback redirect, not the copy-code page.
        assert!(
            param_of(&url, "redirect_uri").starts_with("http://localhost:"),
            "{url}"
        );
        let prompt = prompts.get(1).unwrap();
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
        let interaction = ScriptedInteraction::new(vec![
            Ok(BROWSER_LOGIN_METHOD.to_string()),
            Ok("PASTED_CODE".to_string()),
        ]);

        let cred = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap();
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

    /// Answers the manual prompt with a full redirect URL carrying the flow's own state, built —
    /// like pi `test/anthropic-oauth.test.ts:62-70` — from the `redirect_uri` and `state` of the
    /// `auth_url` event this interaction just received.
    #[derive(Default)]
    struct PasteRedirectUrl {
        auth_url: Mutex<Option<String>>,
    }

    #[async_trait::async_trait]
    impl AuthInteraction for PasteRedirectUrl {
        async fn prompt(&self, prompt: AuthPrompt) -> Result<String, OAuthError> {
            // PROV-120 — the method selector comes first.
            if prompt.kind == Some(AuthPromptKind::Select) {
                return Ok(BROWSER_LOGIN_METHOD.to_string());
            }
            let auth_url = self.auth_url.lock().ok().and_then(|s| s.clone());
            match auth_url {
                Some(url) => Ok(format!(
                    "{}?code=URLCODE&state={}",
                    param_of(&url, "redirect_uri"),
                    param_of(&url, "state")
                )),
                None => Err(OAuthError::Failed("no auth_url seen".to_string())),
            }
        }
        fn notify(&self, event: AuthEvent) {
            if let AuthEvent::AuthUrl { url, .. } = event
                && let Ok(mut slot) = self.auth_url.lock()
            {
                *slot = Some(url);
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
        let cred = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap();
        match cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "url-access"),
            other => panic!("expected oauth credential, got {other:?}"),
        }
        let (_, body) = token.recorded();
        assert_eq!(body.get("code").and_then(|v| v.as_str()), Some("URLCODE"));
        // pi `test/anthropic-oauth.test.ts:43-79` — the pasted callback is exchanged against the
        // same localhost redirect the authorize URL advertised.
        let advertised = param_of(
            interaction.auth_url.lock().unwrap().as_deref().unwrap(),
            "redirect_uri",
        );
        assert!(advertised.starts_with("http://localhost:"), "{advertised}");
        assert_eq!(
            body.get("redirect_uri").and_then(|v| v.as_str()),
            Some(advertised.as_str())
        );
    }

    #[tokio::test]
    async fn login_rejects_pasted_state_mismatch() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![
            Ok(BROWSER_LOGIN_METHOD.to_string()),
            Ok("http://localhost:53692/callback?code=C&state=not-the-verifier".to_string()),
        ]);
        let err = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        // anthropic.ts:279
        assert_eq!(err.to_string(), "OAuth state mismatch");
    }

    #[tokio::test]
    async fn login_rejects_empty_paste_as_missing_code() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![
            Ok(BROWSER_LOGIN_METHOD.to_string()),
            Ok("   ".to_string()),
        ]);
        let err = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        // anthropic.ts:295
        assert_eq!(err.to_string(), "Missing authorization code");
    }

    /// `anthropic.ts:186-194` @v1.1.0+ — `state = parsed.state ?? verifier` and no "Missing OAuth
    /// state" check (pi removed it in `4df157433`, v0.99.0). A paste whose state is present but
    /// empty skips the mismatch check (`:187` @v1.1.0, `""` is falsy), keeps `""` through `??`, and
    /// is exchanged with `state: ""` — in both the `code#` and the `?code=C&state=` spellings.
    #[tokio::test]
    async fn login_exchanges_a_paste_with_an_empty_state_like_upstream() {
        for paste in ["C#", "http://localhost:53692/callback?code=C&state="] {
            let mut token = FakeTokenServer::start(
                200,
                r#"{"access_token":"a","refresh_token":"r","expires_in":3600}"#,
            );
            let oauth = strategy_for(&token.url);
            let interaction = ScriptedInteraction::new(vec![
                Ok(BROWSER_LOGIN_METHOD.to_string()),
                Ok(paste.to_string()),
            ]);
            oauth
                .login(&interaction, &LoginOptions::default())
                .await
                .unwrap();
            let (_, body) = token.recorded();
            assert_eq!(
                body.get("code").and_then(|v| v.as_str()),
                Some("C"),
                "{paste}"
            );
            assert_eq!(
                body.get("state").and_then(|v| v.as_str()),
                Some(""),
                "{paste}"
            );
        }
    }

    #[tokio::test]
    async fn login_propagates_prompt_cancellation() {
        // anthropic.ts:267-270 + :273 — a rejected prompt aborts the whole login. The selector is
        // answered first so it is the manual prompt, not the selector, that is cancelled here.
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![
            Ok(BROWSER_LOGIN_METHOD.to_string()),
            Err(OAuthError::Cancelled),
        ]);
        let err = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Login cancelled");
    }

    /// The `manual_code` prompt's own cancel token, as the login handed it to the interaction.
    fn manual_prompt_cancel(interaction: &ScriptedInteraction) -> CancelToken {
        interaction
            .prompts()
            .into_iter()
            .find(|p| p.kind == Some(AuthPromptKind::ManualCode))
            .and_then(|p| p.cancel)
            .expect("the manual_code prompt carries its own cancel token")
    }

    /// pi `test/anthropic-oauth.test.ts:177-211` @v1.1.0 — "anthropicOAuth.login resolves
    /// through the manual_code prompt and aborts it after settling". The paste wins the race
    /// against the listener, and the prompt's signal is still aborted once the login settles "so
    /// UIs can dismiss it" (`callback-server.ts:180-182` @v1.1.0 aborts in `finally`, whichever
    /// side won).
    #[tokio::test]
    async fn login_resolves_through_the_manual_prompt_and_aborts_it_after_settling() {
        let token = FakeTokenServer::start(
            200,
            r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600}"#,
        );
        let oauth = strategy_for(&token.url);
        let interaction = ScriptedInteraction::new(vec![
            Ok(BROWSER_LOGIN_METHOD.to_string()),
            Ok("the-code".to_string()),
        ]);

        let credential = within(
            "the login",
            oauth.login(&interaction, &LoginOptions::default()),
        )
        .await
        .unwrap();

        match credential {
            Credential::Oauth { access, .. } => assert_eq!(access, "access"),
            other => panic!("expected oauth credential, got {other:?}"),
        }
        assert!(
            interaction
                .events()
                .iter()
                .any(|e| matches!(e, AuthEvent::AuthUrl { .. }))
        );
        assert!(
            manual_prompt_cancel(&interaction).is_cancelled(),
            "the manual prompt's signal is aborted once the login settles"
        );
    }

    /// The same `finally` on the failure exit: a rejected paste still aborts its own prompt.
    #[tokio::test]
    async fn a_failed_manual_prompt_is_aborted_too() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![
            Ok(BROWSER_LOGIN_METHOD.to_string()),
            Err(OAuthError::Failed("prompt failed".to_string())),
        ]);
        let err = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "prompt failed");
        assert!(manual_prompt_cancel(&interaction).is_cancelled());
    }

    /// PROV-137 — the non-settling branches of `callback-server.ts:85-104` @v1.1.0 answer the
    /// browser and keep listening, so a subsequent, correct redirect still completes the login.
    /// The state check comes first, so an `?error=` redirect without the flow's state is a
    /// mismatch (it cannot end the login), and a bare `?error=` is not truthy.
    #[tokio::test]
    async fn bad_redirects_are_answered_without_ending_the_login() {
        let mut token = FakeTokenServer::start(
            200,
            r#"{"access_token":"second-try","refresh_token":"r","expires_in":3600}"#,
        );
        let oauth = strategy_for(&token.url);
        let interaction = Arc::new(
            ScriptedInteraction::new(vec![Ok(BROWSER_LOGIN_METHOD.to_string())])
                .blocking_when_empty(),
        );

        let driver = {
            let interaction = interaction.clone();
            tokio::spawn(async move {
                let (port, state) = await_auth_url(&interaction).await;
                tokio::task::spawn_blocking(move || {
                    // `callback-server.ts:85-88` @v1.1.0 — no state at all, so even a denial is a
                    // mismatch.
                    let stateless_denial = http_get(port, "/callback?error=access_denied");
                    // `callback-server.ts:85-88` @v1.1.0
                    let missing_state = http_get(port, "/callback?code=only");
                    let mismatch = http_get(port, "/callback?code=C&state=wrong");
                    // `callback-server.ts:93-94` @v1.1.0 — `if (error)`: an empty `error` is falsy;
                    // then `:100-104`.
                    let missing_code = http_get(port, &format!("/callback?error=&state={state}"));
                    // …and then the good one.
                    let ok = http_get(port, &format!("/callback?code=GOOD&state={state}"));
                    (stateless_denial, missing_state, mismatch, missing_code, ok)
                })
                .await
                .unwrap()
            })
        };

        let cred = within(
            "the login",
            oauth.login(interaction.as_ref(), &LoginOptions::default()),
        )
        .await
        .unwrap();
        let (stateless_denial, missing_state, mismatch, missing_code, ok) =
            within("the redirects", driver).await.unwrap();

        for page in [&stateless_denial, &missing_state, &mismatch] {
            assert!(page.0.starts_with("HTTP/1.1 400"), "{page:?}");
            assert!(page.1.contains("State mismatch."), "{page:?}");
        }
        assert!(
            missing_code.0.starts_with("HTTP/1.1 400"),
            "{missing_code:?}"
        );
        assert!(
            missing_code.1.contains("Missing authorization code."),
            "{missing_code:?}"
        );

        assert!(ok.0.starts_with("HTTP/1.1 200"), "{ok:?}");
        match cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "second-try"),
            other => panic!("expected oauth credential, got {other:?}"),
        }
        let (_, body) = token.recorded();
        assert_eq!(body.get("code").and_then(|v| v.as_str()), Some("GOOD"));
    }

    /// PROV-137 — `callback-server.ts:93-99` @v1.1.0 (pi `test/oauth-callback-server.test.ts:103-117`
    /// for the shared server): a redirect carrying the flow's state and an OAuth `error` — the user
    /// clicked *Deny* — answers 400 "Anthropic authorization failed." with the description and
    /// **fails the login** with `Anthropic authorization failed: <error_description ?? error>`,
    /// rather than leaving it waiting on the paste prompt (which never answers here, so only the
    /// redirect can end the login). No token exchange is attempted.
    #[tokio::test]
    async fn a_denied_redirect_fails_the_login() {
        for (query, description) in [
            ("error=access_denied", "access_denied"),
            (
                "error=access_denied&error_description=User%20denied%20access",
                "User denied access",
            ),
        ] {
            let token = FakeTokenServer::start(200, r#"{"access_token":"never"}"#);
            let oauth = strategy_for(&token.url);
            let interaction = Arc::new(
                ScriptedInteraction::new(vec![Ok(BROWSER_LOGIN_METHOD.to_string())])
                    .blocking_when_empty(),
            );
            let driver = {
                let interaction = interaction.clone();
                let query = query.to_string();
                tokio::spawn(async move {
                    let (port, state) = await_auth_url(&interaction).await;
                    tokio::task::spawn_blocking(move || {
                        http_get(port, &format!("/callback?{query}&state={state}"))
                    })
                    .await
                    .unwrap()
                })
            };

            let err = within(
                "the login",
                oauth.login(interaction.as_ref(), &LoginOptions::default()),
            )
            .await
            .unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("Anthropic authorization failed: {description}")
            );

            let (status, page) = within("the redirect", driver).await.unwrap();
            assert!(status.starts_with("HTTP/1.1 400"), "{status}");
            assert!(page.contains("Anthropic authorization failed."), "{page}");
            assert!(page.contains(description), "{page}");
            assert_eq!(token.hits(), 0, "a denied login must not exchange a code");
        }
    }

    /// pi `8d8ae2fc2` (#10571), `test/anthropic-oauth.test.ts:224-240` — "falls back to a free
    /// callback port when the preferred port cannot be bound". The preferred port is genuinely
    /// occupied for the whole login (asserted by the control below), so the flow's
    /// `.catch(() => startCallbackServer(0))` (`anthropic.ts:155` @v1.1.0) must bind an OS-chosen
    /// port, advertise it on `localhost`, accept the browser redirect there, and exchange the code
    /// against that same bound redirect URI.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn login_falls_back_to_a_free_port_when_the_preferred_port_cannot_bind() {
        // `listen(blocker, 53692)` — held until the end of the test. Binding `:0` and configuring
        // the flow with the number it got avoids racing another test for a hardcoded port.
        let squatter = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let preferred = squatter.local_addr().unwrap().port();

        // Control: the preferred port really is unbindable, so the listener the login reaches
        // below exists because of the fallback and not because the first bind succeeded.
        let blocked = CallbackServer::start(
            CallbackServerConfig::fixed(preferred, CALLBACK_PATH).with_host("127.0.0.1"),
            AnthropicCallbackHandler {
                expected_state: "s".to_string(),
            },
        )
        .await;
        assert!(
            matches!(blocked, Err(OAuthError::Listen { .. })),
            "an occupied port must fail to bind; got {}",
            if blocked.is_ok() {
                "a listener"
            } else {
                "another error"
            }
        );

        let mut token = FakeTokenServer::start(
            200,
            r#"{"access_token":"fallback-access","refresh_token":"fallback-refresh","expires_in":3600}"#,
        );
        let oauth =
            AnthropicOAuth::with_endpoints(AUTHORIZE_URL, &token.url, "127.0.0.1", preferred);
        // `loginThroughBrowserCallback()` — the manual prompt never answers, so only a redirect
        // delivered to the advertised (fallback) port can complete the login.
        let interaction = Arc::new(
            ScriptedInteraction::new(vec![Ok(BROWSER_LOGIN_METHOD.to_string())])
                .blocking_when_empty(),
        );
        let driver = {
            let interaction = interaction.clone();
            tokio::spawn(async move {
                let (port, state) = await_auth_url(&interaction).await;
                tokio::task::spawn_blocking(move || {
                    http_get(port, &format!("/callback?code=browser-code&state={state}"))
                })
                .await
                .unwrap()
            })
        };

        let cred = within(
            "the login",
            oauth.login(interaction.as_ref(), &LoginOptions::default()),
        )
        .await
        .unwrap();
        let (status, _) = within("the redirect", driver).await.unwrap();
        // `expect(login.pageStatus).toBe(200)`
        assert!(status.starts_with("HTTP/1.1 200"), "{status}");
        match cred {
            Credential::Oauth { access, .. } => assert_eq!(access, "fallback-access"),
            other => panic!("expected oauth credential, got {other:?}"),
        }

        // `redirectUri.hostname` is `localhost`, the path is `/callback`, and the port is NOT the
        // preferred one.
        let url = auth_url_of(&interaction);
        let redirect = param_of(&url, "redirect_uri");
        let port: u16 = redirect
            .strip_prefix("http://localhost:")
            .and_then(|rest| rest.strip_suffix(CALLBACK_PATH))
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| panic!("not a localhost callback URI: {redirect}"));
        assert_ne!(
            port, preferred,
            "the occupied preferred port was advertised"
        );
        assert_ne!(port, 0);

        // `expect(login.exchangedRedirectUri).toBe(login.redirectUri)` (`:194` @v1.1.0), and the
        // paste prompt names the same bound URI (`:179` @v1.1.0).
        let (_, body) = token.recorded();
        assert_eq!(
            body.get("redirect_uri").and_then(|v| v.as_str()),
            Some(redirect.as_str())
        );
        assert_eq!(
            body.get("code").and_then(|v| v.as_str()),
            Some("browser-code")
        );
        let prompts = interaction.prompts();
        let prompt = prompts.get(1).unwrap();
        assert_eq!(prompt.message, MANUAL_PROMPT_MESSAGE);
        assert_eq!(prompt.placeholder.as_deref(), Some(redirect.as_str()));

        drop(squatter);
    }

    /// PROV-117 — `4df157433`, and the last link of `8d8ae2fc2`'s chain: when NEITHER the
    /// preferred port nor an OS-chosen one can be bound, the final `.catch(() => undefined)`
    /// (`anthropic.ts:156` @v1.1.0) leaves no listener and `waitForCallbackOrManualInput` runs the
    /// paste prompt alone — "Without a callback server only the manual prompt is used"
    /// (`callback-server.ts:150-183` @v1.1.0). The login must still complete.
    ///
    /// Both binds fail for real rather than being mocked: the bind host is `192.0.2.117`, a
    /// TEST-NET-1 address (RFC 5737) that is never assigned to a local interface, so every port —
    /// fixed or `0` — fails with `EADDRNOTAVAIL` (the control below asserts it). That stands in
    /// for a sandbox that refuses the loopback listen outright. No other test binds this host,
    /// so `callback::bind_attempts` shows exactly this login's two attempts, in
    /// upstream's order.
    ///
    /// Host assumption: the kernel refuses to bind an address no interface carries — Linux's
    /// default `net.ipv4.ip_nonlocal_bind=0`. A host with `ip_nonlocal_bind=1` (some container and
    /// HA setups) lets the control bind succeed, and the control then fails loudly saying so,
    /// rather than this test passing for the wrong reason.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn login_degrades_to_manual_paste_when_no_callback_port_can_bind() {
        const UNBINDABLE: &str = "192.0.2.117";
        for port in [CALLBACK_PORT, 0] {
            let blocked = CallbackServer::start(
                CallbackServerConfig::fixed(port, CALLBACK_PATH).with_host(UNBINDABLE),
                AnthropicCallbackHandler {
                    expected_state: "s".to_string(),
                },
            )
            .await;
            assert!(
                matches!(blocked, Err(OAuthError::Listen { .. })),
                "{UNBINDABLE}:{port} must fail to bind; this host allows non-local binds \
                 (net.ipv4.ip_nonlocal_bind=1?), which this test cannot run under"
            );
        }
        let before = bind_attempts::on_host(UNBINDABLE).len();

        let mut token = FakeTokenServer::start(
            200,
            r#"{"access_token":"degraded-access","refresh_token":"degraded-refresh","expires_in":3600}"#,
        );
        let oauth =
            AnthropicOAuth::with_endpoints(AUTHORIZE_URL, &token.url, UNBINDABLE, CALLBACK_PORT);
        let interaction = ScriptedInteraction::new(vec![
            Ok(BROWSER_LOGIN_METHOD.to_string()),
            Ok("PASTED_CODE".to_string()),
        ]);

        let cred = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap();
        match cred {
            Credential::Oauth {
                access, refresh, ..
            } => {
                assert_eq!(access, "degraded-access");
                assert_eq!(refresh, "degraded-refresh");
            }
            other => panic!("expected oauth credential, got {other:?}"),
        }

        // The preferred port, then `0`, then nothing (`:154-156` @v1.1.0).
        assert_eq!(
            bind_attempts::on_host(UNBINDABLE)[before..],
            [
                format!("{UNBINDABLE}:{CALLBACK_PORT}"),
                format!("{UNBINDABLE}:0")
            ]
        );

        // `callback?.redirectUri ?? REDIRECT_URI` (`:157` @v1.1.0) — with no listener the authorize
        // URL, the paste prompt and the exchange all name upstream's constant, verbatim.
        let url = auth_url_of(&interaction);
        assert_eq!(param_of(&url, "redirect_uri"), REDIRECT_URI);
        let (_, body) = token.recorded();
        assert_eq!(
            body.get("redirect_uri").and_then(|v| v.as_str()),
            Some(REDIRECT_URI)
        );
        assert_eq!(
            body.get("code").and_then(|v| v.as_str()),
            Some("PASTED_CODE")
        );
        let prompts = interaction.prompts();
        let prompt = prompts.get(1).unwrap();
        assert_eq!(prompt.message, MANUAL_PROMPT_MESSAGE);
        assert_eq!(prompt.placeholder.as_deref(), Some(REDIRECT_URI));
    }

    /// The listener's own URI and [`AnthropicOAuth::redirect_uri`] (the no-listener fallback) are
    /// composed the same way — advertise host, port, path — so for any port the bound and the
    /// unbound spellings agree, and the production composition is upstream's `REDIRECT_URI`
    /// (`anthropic.ts:22` @v1.1.0) verbatim. The port comes from an ephemeral bind the server still
    /// holds, so nothing is released and re-bound in between.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn redirect_uri_matches_the_bound_listener() {
        assert_eq!(
            AnthropicOAuth::new().redirect_uri(),
            REDIRECT_URI,
            "the production composition is upstream's constant verbatim"
        );

        let server = CallbackServer::start(
            CallbackServerConfig::ephemeral(CALLBACK_PATH)
                .with_host("127.0.0.1")
                .advertising(ADVERTISE_HOST),
            AnthropicCallbackHandler {
                expected_state: "s".to_string(),
            },
        )
        .await
        .unwrap();
        let oauth =
            AnthropicOAuth::with_endpoints(AUTHORIZE_URL, TOKEN_URL, "127.0.0.1", server.port());
        assert_eq!(server.redirect_uri(), oauth.redirect_uri());
        server.close();
    }

    // -- PROV-120: method selector + copy-code login, anthropic.ts:200-235 / :282-299 @v1.1.0 --

    /// pi `test/anthropic-oauth.test.ts:121-129` and `:133-143` — the selector is the first
    /// prompt, with upstream's message, ids, labels and order; cancelling it ends the login before
    /// either flow starts.
    #[tokio::test]
    async fn login_offers_browser_first_then_copy_code() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![Err(OAuthError::Cancelled)]);
        let err = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Login cancelled");

        let prompts = interaction.prompts();
        assert_eq!(
            prompts.len(),
            1,
            "nothing is prompted after a cancelled select"
        );
        assert_eq!(prompts[0].kind, Some(AuthPromptKind::Select));
        assert_eq!(prompts[0].message, "Select Anthropic login method:");
        assert_eq!(
            prompts[0].options,
            vec![
                AuthSelectOption {
                    id: "browser".to_string(),
                    label: "Browser login (default)".to_string(),
                    description: None,
                },
                AuthSelectOption {
                    id: "copy_code".to_string(),
                    label: "Copy code login (headless)".to_string(),
                    description: None,
                },
            ]
        );
        assert!(
            interaction.events().is_empty(),
            "no authorize URL was shown"
        );
    }

    /// `anthropic.ts:295-297` @v1.1.0.
    #[tokio::test]
    async fn login_rejects_an_unknown_method() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![Ok("carrier-pigeon".to_string())]);
        let err = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Unknown Anthropic login method: carrier-pigeon"
        );
        assert_eq!(interaction.prompts().len(), 1);
        assert!(interaction.events().is_empty());
    }

    /// Picks `copy_code`, then pastes what Anthropic's hosted page would show — built from the
    /// `auth_url` event's own state by `paste`.
    struct CopyCodePaste {
        paste: fn(&str) -> String,
        state: Mutex<Option<String>>,
        prompts: Mutex<Vec<AuthPrompt>>,
        events: Mutex<Vec<AuthEvent>>,
    }

    impl CopyCodePaste {
        fn new(paste: fn(&str) -> String) -> Self {
            CopyCodePaste {
                paste,
                state: Mutex::new(None),
                prompts: Mutex::new(Vec::new()),
                events: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl AuthInteraction for CopyCodePaste {
        async fn prompt(&self, prompt: AuthPrompt) -> Result<String, OAuthError> {
            let kind = prompt.kind;
            self.prompts.lock().unwrap().push(prompt);
            if kind == Some(AuthPromptKind::Select) {
                return Ok(COPY_CODE_LOGIN_METHOD.to_string());
            }
            let state = self.state.lock().unwrap().clone();
            match state {
                Some(state) => Ok((self.paste)(&state)),
                None => Err(OAuthError::Failed("no auth_url seen".to_string())),
            }
        }
        fn notify(&self, event: AuthEvent) {
            if let AuthEvent::AuthUrl { url, .. } = &event {
                *self.state.lock().unwrap() = Some(param_of(url, "state"));
            }
            self.events.lock().unwrap().push(event);
        }
    }

    /// The whole copy-code login — `anthropic.ts:200-235` @v1.1.0, pi
    /// `test/anthropic-oauth.test.ts:81-131`.
    ///
    /// "Starts no listener" is proven from the callback server's own record of start calls
    /// (`callback::bind_attempts`), not by probing a port. A probe is racy whichever way it is
    /// built: bind-release-rebind of a "free" port can lose it to a parallel test; and holding the
    /// configured port for the whole login proves nothing since `8d8ae2fc2`, because a flow that
    /// did try to listen would just fall back to an OS-chosen port nobody is watching. The record
    /// sees every attempt — preferred port, fallback, or a bind that failed and was swallowed —
    /// and is scoped to a bind host (`192.0.2.120`, TEST-NET-1) that no other test uses, so it is
    /// deterministic under parallel test execution. The control at the end shows the record does
    /// see a start on that host, so the empty result is not vacuous.
    #[tokio::test]
    async fn copy_code_login_exchanges_against_the_copy_code_redirect() {
        const SENTINEL_HOST: &str = "192.0.2.120";
        let mut token = FakeTokenServer::start(
            200,
            r#"{"access_token":"copied-access","refresh_token":"copied-refresh","expires_in":3600}"#,
        );
        let oauth =
            AnthropicOAuth::with_endpoints(AUTHORIZE_URL, &token.url, SENTINEL_HOST, CALLBACK_PORT);
        let interaction = CopyCodePaste::new(|state| format!("copied-code#{state}"));

        let cred = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap();
        match cred {
            Credential::Oauth {
                access, refresh, ..
            } => {
                assert_eq!(access, "copied-access");
                assert_eq!(refresh, "copied-refresh");
            }
            other => panic!("expected oauth credential, got {other:?}"),
        }

        // No callback listener: not on the preferred port, not on a fallback, not even a failed
        // attempt.
        assert_eq!(
            bind_attempts::on_host(SENTINEL_HOST),
            Vec::<String>::new(),
            "the copy-code login must not start a callback listener"
        );

        // `:215` then `:227` @v1.1.0 — the two notifications, in order.
        let events = interaction.events.lock().unwrap().clone();
        assert_eq!(events.len(), 2, "{events:?}");
        let url = match &events[0] {
            AuthEvent::AuthUrl { url, instructions } => {
                assert_eq!(
                    instructions.as_deref(),
                    Some(
                        "Complete login in your browser, then copy the code Anthropic shows and paste it here."
                    )
                );
                url.clone()
            }
            other => panic!("expected auth_url first, got {other:?}"),
        };
        assert_eq!(
            events[1],
            AuthEvent::Progress {
                message: "Exchanging authorization code for tokens...".to_string()
            }
        );
        // `:202-211` @v1.1.0 — the authorize URL carries the copy-code redirect.
        assert!(url.starts_with(&format!("{AUTHORIZE_URL}?")), "{url}");
        assert_eq!(param_of(&url, "redirect_uri"), COPY_CODE_REDIRECT_URI);
        let state = param_of(&url, "state");

        // `:218-223` @v1.1.0 — one paste prompt after the selector.
        let prompts = interaction.prompts.lock().unwrap().clone();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[1].kind, Some(AuthPromptKind::ManualCode));
        assert_eq!(
            prompts[1].message,
            "Paste the code Anthropic shows after you sign in:"
        );
        assert_eq!(prompts[1].placeholder.as_deref(), Some("code#state"));

        // `:228-234` @v1.1.0 — the exchange, against the copy-code redirect.
        let (_, body) = token.recorded();
        assert_eq!(
            body.get("redirect_uri").and_then(|v| v.as_str()),
            Some(COPY_CODE_REDIRECT_URI)
        );
        assert_eq!(
            body.get("code").and_then(|v| v.as_str()),
            Some("copied-code")
        );
        assert_eq!(
            body.get("state").and_then(|v| v.as_str()),
            Some(state.as_str())
        );
        assert_eq!(
            body.get("code_verifier").and_then(|v| v.as_str()),
            Some(state.as_str())
        );

        // Control: a start on the sentinel host IS recorded, so the empty record above means
        // "never called", not "not observed". The record is written before the bind, so this
        // holds whether or not the host lets an unassigned address bind (TEST-NET-1 normally
        // fails with `EADDRNOTAVAIL`; `net.ipv4.ip_nonlocal_bind=1` lets it succeed).
        let control = CallbackServer::start(
            CallbackServerConfig::fixed(CALLBACK_PORT, CALLBACK_PATH).with_host(SENTINEL_HOST),
            AnthropicCallbackHandler {
                expected_state: "s".to_string(),
            },
        )
        .await;
        drop(control);
        assert_eq!(
            bind_attempts::on_host(SENTINEL_HOST),
            vec![format!("{SENTINEL_HOST}:{CALLBACK_PORT}")]
        );
    }

    /// `anthropic.ts:218-223` @v1.1.0 — the paste prompt carries the login-wide cancel
    /// (`signal: interaction.signal`), and a rejected paste aborts the login.
    #[tokio::test]
    async fn copy_code_login_propagates_prompt_cancellation() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let cancel = CancelToken::new();
        let interaction = ScriptedInteraction::new(vec![
            Ok(COPY_CODE_LOGIN_METHOD.to_string()),
            Err(OAuthError::Cancelled),
        ])
        .with_cancel(cancel);
        let err = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Login cancelled");
        let prompts = interaction.prompts();
        assert_eq!(prompts.len(), 2);
        assert!(
            prompts[1].cancel.is_some(),
            "the paste prompt must carry the cancel token"
        );
    }

    /// `anthropic.ts:225` @v1.1.0.
    #[tokio::test]
    async fn copy_code_login_rejects_state_mismatch() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![
            Ok(COPY_CODE_LOGIN_METHOD.to_string()),
            Ok("C#not-the-verifier".to_string()),
        ]);
        let err = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "OAuth state mismatch");
        assert_eq!(
            param_of(&auth_url_of(&interaction), "redirect_uri"),
            COPY_CODE_REDIRECT_URI
        );
    }

    /// `anthropic.ts:226` @v1.1.0.
    #[tokio::test]
    async fn copy_code_login_rejects_empty_paste_as_missing_code() {
        let oauth = strategy_for("http://127.0.0.1:1/never-called");
        let interaction = ScriptedInteraction::new(vec![
            Ok(COPY_CODE_LOGIN_METHOD.to_string()),
            Ok("   ".to_string()),
        ]);
        let err = oauth
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Missing authorization code");
    }

    /// `anthropic.ts:228-234` @v1.1.0 — `parsed.state ?? verifier`, with no "Missing OAuth state"
    /// check (upstream dropped it from both flows in `4df157433`): a bare code exchanges with the
    /// verifier as state, and `code#` sends `""` — as the browser flow does too
    /// (`login_exchanges_a_paste_with_an_empty_state_like_upstream`).
    #[tokio::test]
    async fn copy_code_login_defaults_state_like_upstream() {
        for (paste, expect_empty_state) in [("BARE", false), ("BARE#", true)] {
            let mut token = FakeTokenServer::start(
                200,
                r#"{"access_token":"a","refresh_token":"r","expires_in":3600}"#,
            );
            let oauth = strategy_for(&token.url);
            let interaction = ScriptedInteraction::new(vec![
                Ok(COPY_CODE_LOGIN_METHOD.to_string()),
                Ok(paste.to_string()),
            ]);
            oauth
                .login(&interaction, &LoginOptions::default())
                .await
                .unwrap();
            let (_, body) = token.recorded();
            let verifier = param_of(&auth_url_of(&interaction), "state");
            let expected = if expect_empty_state {
                ""
            } else {
                verifier.as_str()
            };
            assert_eq!(body.get("code").and_then(|v| v.as_str()), Some("BARE"));
            assert_eq!(
                body.get("state").and_then(|v| v.as_str()),
                Some(expected),
                "{paste}"
            );
        }
    }
}
