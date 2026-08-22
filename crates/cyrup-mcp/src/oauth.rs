//! The OAuth 2.1 flow and the loopback callback listener — `mcp-auth-flow.ts`,
//! `mcp-oauth-provider.ts`, `mcp-callback-server.ts` (gap-analysis `13g-mcp-oauth.md`,
//! MCP-300 … MCP-349).
//!
//! This module is what lets an HTTP MCP server answer *"you are not authorized"* and have the
//! adapter turn that into a browser round trip, a token in the OS credential store, and a working
//! connection — without the user configuring an endpoint.
//!
//! # The split: `rmcp` owns the protocol, `cyrup-mcp` owns the flow around it
//!
//! The four RFCs in this subsystem's title are **not** implemented here, and they were not
//! implemented upstream either — upstream they live inside `@modelcontextprotocol/client`'s
//! `auth()` orchestrator, and the adapter's job is to prime it, hand it a provider, and intercept
//! the one place it hands control back. In the port that orchestrator is `rmcp`'s `auth` feature
//! ([`rmcp::transport::auth`]): RFC 9728 protected-resource metadata and RFC 8414/OIDC
//! authorization-server discovery, RFC 7591 dynamic client registration, PKCE S256 (there is no
//! `plain` path), the RFC 8707 `resource` parameter on every leg, the RFC 9207 `iss` gate,
//! SEP-2207 `offline_access` augmentation, automatic refresh, and the client-credentials grant.
//! **This module hand-writes no OAuth protocol code at all.**
//!
//! What it *does* hand-write is everything the adapter itself wrote: flow ownership and generation
//! fencing, config validation and its exact messages, the loopback listener's multiplexing and
//! lifetime, the manual/headless paste leg, the stale-registration hygiene, and the user-facing
//! strings.
//!
//! # The listener is a reuse, not a rebuild (MCP-336)
//!
//! [`cyrup_provider::auth::oauth::callback`] already ships a real loopback listener:
//! `CallbackServer::start(CallbackServerConfig, handler)` binds a `std::net::TcpListener`, runs a
//! detached accept thread with a request-read timeout and a request-head cap, routes one path, and
//! hands each request to a `CallbackHandler` returning `CallbackOutcome::{Complete, Failed,
//! Continue}`. **`Continue` is the seam that makes it multi-tenant**: a handler that never calls
//! `CallbackControl::claim` and always returns `Continue` never settles the server's own one-shot,
//! so one listener serves N concurrent logins while this module's `state`→waiter map does the
//! routing inside the handler.
//!
//! Two named deltas come from the reuse, both cosmetic: a request for a foreign path is answered
//! by `serve_connection` with the HTML error page at 404 rather than `text/plain` `Not found`, and
//! a malformed request gets a 400 HTML page upstream has no branch for (Node's parser handles it
//! below the handler).
//!
//! # The one structural consequence of `rmcp` owning CSRF generation
//!
//! Upstream generates the `state` nonce itself ([`generate_state`], 64 lowercase hex characters)
//! and the SDK asks the provider for it. `rmcp` generates its own CSRF token inside
//! `get_authorization_url`, and there is no API to feed it ours — so the browser will come back
//! carrying **rmcp's** token, not ours. [`start_auth`] therefore reserves the callback listener
//! under its own nonce (the reservation has to exist *before* the bind, and the URL does not exist
//! yet), then re-keys the reservation onto rmcp's token the moment the authorization URL is
//! known — see [`start_auth`]. Everything user-visible afterwards (`pendingAuthStates`,
//! `cancelPendingCallback`, the manual paste's expected-state comparison) uses rmcp's token.
//! The swap window is entirely inside `start_auth`, before any browser is opened.
//!
//! # Storage is a seam, not a dependency
//!
//! Token storage itself — the OS keyring, the chunking manifest, the `keyctl` recovery — is
//! `mcp-auth.ts` and belongs to gap-analysis section 13f ([`crate::credentials`]). This module
//! consumes it through the [`McpOAuthStorage`] trait so the two can land independently;
//! [`InMemoryOAuthStorage`] is the test double and the interim default. See the module's report
//! for the one-adapter wiring that binds 13f's `McpAuthStore` to this trait.
//!
//! # Everything here is `extension-owned`
//!
//! `crates/cyrup-mcp` is a native built-in crate compiled into the binary, not a WASM guest; it
//! links `rmcp`, `keyring` and `opener` directly and runs `std::process::Command` itself. The
//! `HostServices::exec` gate is the *guest* capability model and does not apply here, which is why
//! [`crate::secrets::resolve_command_secret`] spawns a shell directly and [`OpenerLauncher`] calls
//! `opener::open`.
//! The prompt/notify verbs a command surface needs are injected as callbacks
//! ([`AuthenticateOptions::on_authorization_input`]) so this module never names `HostServices`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cyrup_core::CancelToken;
use futures::FutureExt as _;
use futures::future::{BoxFuture, Shared};
use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationMetadata, AuthorizationRequest,
    AuthorizationSession, ClientCredentialsConfig, CredentialStore, InMemoryStateStore,
    OAuthClientConfig, OAuthState, StoredCredentials,
};
use tokio::sync::{Mutex as AsyncMutex, oneshot};

use crate::abort::{abortable, is_abort_error, throw_if_aborted};
use crate::config::{AuthKind, AuthMode, OAuthGrantType, OAuthSetting, RawJson, ServerEntry};
use crate::dirs::{APP_CLIENT_URI, APP_NAME};
use crate::errors::{McpError, McpResult};

// ===================================================================================================
// 1 · Environment interpolation and `!command` secrets — `utils.ts` (MCP-342, MCP-349)
// ===================================================================================================
//
// The engine moved to [`crate::secrets`] when MCP-083's other two call sites landed. It was written
// here because the OAuth `clientSecret` was its first caller, but `utils.ts` is imported by three
// subsystems, and leaving it here would have made the stdio transport depend on the OAuth flow for
// a shell spawn. Every name below is re-exported rather than relocated so `crate::oauth::…` paths —
// including `request_headers_command`'s `use crate::oauth::interpolate_env_vars` — keep resolving,
// and so this section still reads as the place `utils.ts` landed.

pub use crate::secrets::{
    interpolate_env_vars, interpolate_env_vars_with, interpolate_secret_expression,
    resolve_command_secret, COMMAND_SECRET_MAX_OUTPUT_BYTES, COMMAND_SECRET_TIMEOUT,
};

// ===================================================================================================
// 2 · `extractOAuthConfig` — the validation gate and its twelve messages (MCP-302, MCP-332)
// ===================================================================================================

/// The runtime-resolved `oauth` block — upstream `McpOAuthConfig` (`mcp-oauth-provider.ts:108`).
///
/// This is [`crate::config::OAuthConfig`] *after* environment interpolation and validation: the
/// on-disk shape is what MCP-069 types, this is what the flow runs on. `client_secret` is the one
/// field that is **not** eagerly interpolated when it carries a `!` command expression, so
/// [`resolve_command_secret`] can still see the expression later.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthFlowConfig {
    /// `authorization_code` (the default) or `client_credentials`. Copied verbatim and, as
    /// upstream, **not** validated here.
    pub grant_type: Option<OAuthGrantType>,
    /// A pre-registered client id. Its presence is half of the `strictPort` predicate.
    pub client_id: Option<String>,
    /// The confidential-client secret, possibly still a `!command` expression.
    pub client_secret: Option<String>,
    /// Requested scopes, space-separated in one string (upstream's shape, not an array).
    pub scope: Option<String>,
    /// Extra authorization-URL parameters, already interpolated.
    pub authorization_params: Option<std::collections::BTreeMap<String, String>>,
    /// The exact redirect URI a pre-registered client is registered with; trimmed and non-empty.
    pub redirect_uri: Option<String>,
    /// Client display name for dynamic registration.
    pub client_name: Option<String>,
    /// Client homepage for dynamic registration.
    pub client_uri: Option<String>,
    /// Logo shown on the provider's consent screen; guaranteed to be an absolute `http(s)` URL.
    pub logo_uri: Option<String>,
    /// The security-weakening escape hatch for known-misconfigured authorization servers.
    pub skip_issuer_metadata_validation: Option<bool>,
}

impl OAuthFlowConfig {
    /// `config.grantType === "client_credentials"`.
    #[must_use]
    pub fn uses_client_credentials(&self) -> bool {
        self.grant_type == Some(OAuthGrantType::ClientCredentials)
    }

    /// `strictPort = Boolean(config.clientId) || config.redirectUri !== undefined`
    /// (`mcp-auth-flow.ts:369`).
    ///
    /// When false — the default, dynamic-registration case — the listener binds port `0` and
    /// adopts whatever the OS assigns; **the default flow never binds 19876**.
    #[must_use]
    pub fn strict_port(&self) -> bool {
        self.client_id.as_ref().is_some_and(|id| !id.is_empty()) || self.redirect_uri.is_some()
    }
}

/// `extractOAuthConfig(definition)` (`mcp-auth-flow.ts:151`) — MCP-302.
///
/// Evaluated in source order; every check is a bare error whose message is the contract (13g §5).
/// `oauth === false` yields an empty config, exactly as upstream does — the *disabling* is
/// [`supports_oauth`]'s job, not this function's.
///
/// **Named delta.** Upstream's nine `typeof … !== "string"` / `!== "boolean"` guards cannot fire
/// against [`crate::config::OAuthConfig`], because `config.rs`'s `lenient` deserializer has already
/// degraded a wrong-typed field to `None` by the time this runs. Those twelve messages are
/// preserved in [`validate_oauth_block`], which runs over the **raw** [`RawJson`] at config-load
/// time and is where a malformed `oauth` block must fail — before any network traffic. The
/// value-shaped checks (`must not be empty`, the `logoUri` absoluteness rule, the empty
/// `authorizationParams` key) survive typing and are enforced here.
///
/// **Named delta.** `authorizationParams` is a `BTreeMap` in the typed model where upstream's
/// `Object.entries` yields insertion order, so when two entries are *both* invalid the port reports
/// the alphabetically-first one where upstream reports the first written. Only the reported message
/// differs; every individual rejection is identical.
pub fn extract_oauth_config(definition: &ServerEntry) -> McpResult<OAuthFlowConfig> {
    let raw = match &definition.oauth {
        // `if (definition.oauth === false) return {}` — and the `oauth: true` value that
        // TypeScript's structural cast tolerates lands here too, since it satisfies nothing.
        None | Some(OAuthSetting::Disabled(_)) => return Ok(OAuthFlowConfig::default()),
        Some(OAuthSetting::Config(config)) => config,
    };

    let mut config = OAuthFlowConfig {
        grant_type: raw.grant_type,
        ..OAuthFlowConfig::default()
    };

    if let Some(client_id) = &raw.client_id {
        config.client_id = Some(interpolate_env_vars(client_id));
    }
    if let Some(client_secret) = &raw.client_secret {
        // Preserve command expressions for the resolver; interpolation stays eager for ordinary
        // values (`mcp-auth-flow.ts:163`).
        config.client_secret = Some(if client_secret.starts_with('!') {
            client_secret.clone()
        } else {
            interpolate_env_vars(client_secret)
        });
    }
    if let Some(scope) = &raw.scope {
        config.scope = Some(interpolate_env_vars(scope));
    }
    if let Some(params) = &raw.authorization_params {
        let mut resolved = std::collections::BTreeMap::new();
        for (key, value) in params {
            if key.is_empty() {
                return Err(McpError::other(
                    "OAuth authorizationParams keys must not be empty",
                ));
            }
            resolved.insert(key.clone(), interpolate_env_vars(value));
        }
        config.authorization_params = Some(resolved);
    }
    if let Some(redirect_uri) = &raw.redirect_uri {
        let redirect_uri = interpolate_env_vars(redirect_uri).trim().to_string();
        if redirect_uri.is_empty() {
            return Err(McpError::other("OAuth redirectUri must not be empty"));
        }
        config.redirect_uri = Some(redirect_uri);
    }
    if let Some(client_name) = &raw.client_name {
        let client_name = interpolate_env_vars(client_name).trim().to_string();
        if client_name.is_empty() {
            return Err(McpError::other("OAuth clientName must not be empty"));
        }
        config.client_name = Some(client_name);
    }
    if let Some(client_uri) = &raw.client_uri {
        let client_uri = interpolate_env_vars(client_uri).trim().to_string();
        if client_uri.is_empty() {
            return Err(McpError::other("OAuth clientUri must not be empty"));
        }
        config.client_uri = Some(client_uri);
    }
    if let Some(logo_uri) = &raw.logo_uri {
        let logo_uri = interpolate_env_vars(logo_uri).trim().to_string();
        if logo_uri.is_empty() {
            return Err(McpError::other("OAuth logoUri must not be empty"));
        }
        // Consent screens fetch this server-side, so a relative or `file:` path silently renders
        // nothing. Fail here instead, where the message can say why. Both the parse failure and
        // the scheme failure emit the identical string (`mcp-auth-flow.ts:225`, `:229`).
        let parsed = url::Url::parse(&logo_uri)
            .map_err(|_| McpError::other("OAuth logoUri must be an absolute http(s) URL"))?;
        if parsed.scheme() != "https" && parsed.scheme() != "http" {
            return Err(McpError::other(
                "OAuth logoUri must be an absolute http(s) URL",
            ));
        }
        config.logo_uri = Some(logo_uri);
    }
    config.skip_issuer_metadata_validation = raw.skip_issuer_metadata_validation;

    Ok(config)
}

/// The nine type guards of `extractOAuthConfig`, run over the **raw** `oauth` member before
/// `config.rs`'s `lenient` deserializer can silently drop a wrong-typed field (MCP-302).
///
/// Twelve messages, in upstream's source order. Call this from the config loader with the raw
/// `mcpServers.<name>.oauth` value; a malformed block must fail at config time with a message
/// naming the exact field, before any network traffic. The value-shaped rules that survive typing
/// live in [`extract_oauth_config`] and are checked there instead of duplicated here.
pub fn validate_oauth_block(raw: &RawJson) -> McpResult<()> {
    let entries = match raw {
        RawJson::Bool(false) => return Ok(()),
        RawJson::Object(entries) => entries,
        // Every other shape (including `oauth: true`) satisfies nothing downstream and upstream's
        // optional chaining reads no fields off it.
        _ => return Ok(()),
    };

    let string_field = |key: &str, message: &'static str| -> McpResult<()> {
        match entries.get(key) {
            None | Some(RawJson::Null) => Ok(()),
            Some(RawJson::String(_)) => Ok(()),
            Some(_) => Err(McpError::other(message)),
        }
    };

    string_field("clientId", "OAuth clientId must be a string")?;
    string_field("clientSecret", "OAuth clientSecret must be a string")?;
    string_field("scope", "OAuth scope must be a string")?;

    match entries.get("authorizationParams") {
        None | Some(RawJson::Null) => {}
        Some(RawJson::Object(params)) => {
            for (key, value) in params {
                if key.is_empty() {
                    return Err(McpError::other(
                        "OAuth authorizationParams keys must not be empty",
                    ));
                }
                if !matches!(value, RawJson::String(_)) {
                    return Err(McpError::other(format!(
                        "OAuth authorizationParams.{key} must be a string"
                    )));
                }
            }
        }
        Some(_) => {
            return Err(McpError::other(
                "OAuth authorizationParams must be an object",
            ));
        }
    }

    string_field("redirectUri", "OAuth redirectUri must be a string")?;
    string_field("clientName", "OAuth clientName must be a string")?;
    string_field("clientUri", "OAuth clientUri must be a string")?;
    string_field("logoUri", "OAuth logoUri must be a string")?;

    match entries.get("skipIssuerMetadataValidation") {
        None | Some(RawJson::Null) | Some(RawJson::Bool(_)) => {}
        Some(_) => {
            return Err(McpError::other(
                "OAuth skipIssuerMetadataValidation must be a boolean",
            ));
        }
    }

    Ok(())
}

/// `supportsOAuth(definition)` (`mcp-auth-flow.ts:920`) — MCP-332.
///
/// The truth table's **order is observable** and is reproduced branch for branch:
///
/// | condition, in order | result |
/// | --- | --- |
/// | no `url` | `false` |
/// | `auth === false` | `false` |
/// | `oauth === false` | `false` |
/// | `auth === "oauth"` | `true` — **even with custom headers** |
/// | `headers` present and non-empty | `false` |
/// | `auth === undefined` | `true` |
/// | otherwise (`auth === "bearer"`) | `false` |
///
/// The `auth === "oauth"` row **beats** the custom-headers row. This predicate decides whether a
/// 401 becomes `needs-auth` or a hard error, whether `/mcp-auth` is offered, and whether auto-auth
/// fires; a wrong answer silently disables OAuth for a whole class of servers.
#[must_use]
pub fn supports_oauth(definition: &ServerEntry) -> bool {
    if definition.url.is_none() {
        return false;
    }
    if definition.auth == Some(AuthMode::Disabled(false)) {
        return false;
    }
    if matches!(definition.oauth, Some(OAuthSetting::Disabled(false))) {
        return false;
    }
    if definition.auth == Some(AuthMode::Named(AuthKind::Oauth)) {
        return true;
    }
    if definition
        .headers
        .as_ref()
        .is_some_and(|headers| !headers.is_empty())
    {
        return false;
    }
    definition.auth.is_none()
}

// ===================================================================================================
// 3 · `parseOAuthRedirectUri` — loopback-only redirect validation (MCP-303)
// ===================================================================================================

/// What [`parse_oauth_redirect_uri`] yields — upstream's
/// `{ port, callbackHost, callbackPath }` (`mcp-auth-flow.ts:283`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectEndpoint {
    /// The explicit numeric port. There is no default: an implicit port is rejected.
    pub port: u16,
    /// The host to bind, with `[::1]` already **unbracketed** to `::1` because the bind call wants
    /// a bare literal.
    pub callback_host: String,
    /// The path the listener must serve.
    pub callback_path: String,
}

/// `parseOAuthRedirectUri(redirectUri)` (`mcp-auth-flow.ts:283`) — MCP-303.
///
/// Six ordered checks, five distinct messages. **The order is observable:** a non-loopback URL that
/// also carries a fragment reports the loopback error, not the fragment error.
///
/// Note the allowlist asymmetry: only `localhost`, `127.0.0.1`, `[::1]` and `::1` pass —
/// `127.0.0.2`, `0.0.0.0` and every other `127/8` address are rejected, narrower than RFC 8252
/// §7.3. The TypeScript SDK's own `isLoopbackHost` uses the identical four literals; they agree.
///
/// `url::Url` normalises an IPv6 host to the bracketed form, so the `::1` spelling arrives here as
/// `[::1]` and step 7's unbracketing covers both spellings.
pub fn parse_oauth_redirect_uri(redirect_uri: &str) -> McpResult<RedirectEndpoint> {
    let url = url::Url::parse(redirect_uri)
        .map_err(|_| McpError::other(format!("Invalid OAuth redirectUri: {redirect_uri}")))?;

    let hostname = url.host_str().unwrap_or_default().to_lowercase();
    let is_localhost = matches!(hostname.as_str(), "localhost" | "127.0.0.1" | "[::1]" | "::1");
    if url.scheme() != "http" || !is_localhost {
        return Err(McpError::other(
            "OAuth redirectUri must be an http:// localhost or loopback URI",
        ));
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(McpError::other(
            "OAuth redirectUri must not include username or password",
        ));
    }

    if url.fragment().is_some() {
        return Err(McpError::other(
            "OAuth redirectUri must not include a fragment",
        ));
    }

    // `url::Url::port()` returns `None` for both "no port" and "the scheme's default port", and
    // `http://localhost:80/cb` must keep working, so the presence test reads the raw authority
    // rather than the parsed port. Steps 5 and 6 emit the same message anyway.
    let explicit_port = url.port();
    let Some(port) = explicit_port else {
        let authority_has_port = url
            .authority()
            .rsplit_once(':')
            .is_some_and(|(_, tail)| !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()));
        if authority_has_port {
            // `http://localhost:80` — the default port for the scheme, which upstream's
            // `url.port` reports as `""` and rejects. Reproduced.
            return Err(McpError::other(
                "OAuth redirectUri must include an explicit numeric port",
            ));
        }
        return Err(McpError::other(
            "OAuth redirectUri must include an explicit numeric port",
        ));
    };
    if port == 0 {
        return Err(McpError::other(
            "OAuth redirectUri must include an explicit numeric port",
        ));
    }

    let callback_host = if hostname == "[::1]" {
        "::1".to_string()
    } else {
        hostname
    };
    Ok(RedirectEndpoint {
        port,
        callback_host,
        callback_path: url.path().to_string(),
    })
}

// **The IPv6 re-bracketing copy is gone — the reuse already covers it (MCP-336).**
// [`parse_oauth_redirect_uri`] step 7 hands `EnsureCallbackServer` an *unbracketed* `::1` because
// that is what `TcpListener::bind` wants; the advertised URL is then built by
// `cyrup_provider::auth::oauth::callback`, whose own `bracket_host`
// (`crates/cyrup-provider/src/auth/oauth/callback.rs:350`) re-brackets the advertise host before it
// reaches `redirect_uri()`. A local copy therefore had no production caller, only a test. That
// helper being private in `cyrup-provider` is not a reuse blocker after all, so the report's
// visibility ask is withdrawn.

// ===================================================================================================
// 4 · The three callback pages, including host branding (MCP-307)
// ===================================================================================================

/// `getAppName()` for the pages and for dynamic client registration.
///
/// A distribution that rebrands should register — and greet its users — under its own name;
/// otherwise every consent screen asks them to authorize an app they have never run. The value is
/// [`crate::dirs::APP_NAME`], with a process-global override that exists so MCP-307's branding
/// cases are assertable without a fabricated package manifest. When `dirs` grows a dynamic
/// `getAppName()` this function becomes a one-line delegation.
#[must_use]
pub fn app_name() -> String {
    if let Some(slot) = APP_NAME_OVERRIDE.get()
        && let Ok(guard) = slot.lock()
        && let Some(name) = guard.as_ref()
    {
        return name.clone();
    }
    APP_NAME.to_string()
}

static APP_NAME_OVERRIDE: OnceLock<StdMutex<Option<String>>> = OnceLock::new();

/// Override [`app_name`] for the current process. Intended for the branding tests and for a host
/// that resolves its own name after startup — upstream builds every page per request for exactly
/// that reason.
pub fn set_app_name(name: Option<String>) {
    let slot = APP_NAME_OVERRIDE.get_or_init(|| StdMutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = name;
    }
}

/// `defaultClientUri()` (`mcp-oauth-provider.ts:58`) — the client homepage advertised during
/// dynamic client registration.
///
/// RFC 7591 defines `client_uri` as the home page **of the client**, so a rebranded distribution
/// must not advertise this adapter's repository. Upstream omits the field entirely when it cannot
/// name one; cyrup declares [`crate::dirs::APP_CLIENT_URI`], so the omit arm is unreachable here
/// and is recorded rather than reproduced.
#[must_use]
pub fn default_client_uri() -> Option<String> {
    Some(APP_CLIENT_URI.to_string())
}

/// `escapeHtml(value)` (`mcp-callback-server.ts:120`) — `&`, `<`, `>`, `"`, `'` in that order.
///
/// Identical to `cyrup_provider::auth::oauth::page::escape_html`, which is **private**; the report
/// carries the visibility ask. The order matters: `&` first, or the later replacements' own
/// ampersands would be double-escaped.
#[must_use]
fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// `PAGE_STYLE` (`mcp-callback-server.ts:28`) — inline CSS, dark by default with a
/// `prefers-color-scheme: light` override. Self-contained: no webfont, no external asset.
const PAGE_STYLE: &str = r#"
    :root { color-scheme: light dark; }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 24px;
      font: 15px/1.55 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
      background: #0f1117;
      color: #e6e8ee;
    }
    .card {
      width: 100%;
      max-width: 26rem;
      padding: 2.5rem 2rem;
      text-align: center;
      background: #161922;
      border: 1px solid #242938;
      border-radius: 14px;
      box-shadow: 0 1px 2px rgba(0,0,0,.3), 0 12px 32px rgba(0,0,0,.25);
    }
    .badge {
      width: 3rem; height: 3rem;
      margin: 0 auto 1.25rem;
      display: grid; place-items: center;
      border-radius: 50%;
    }
    .badge svg { width: 1.5rem; height: 1.5rem; display: block; }
    .ok   { background: rgba(74,222,128,.12); color: #4ade80; }
    .bad  { background: rgba(248,113,113,.12); color: #f87171; }
    h1 { margin: 0 0 .5rem; font-size: 1.15rem; font-weight: 600; letter-spacing: -0.01em; }
    p  { margin: 0; color: #9aa1b1; }
    .app { color: #e6e8ee; font-weight: 500; }
    .hint { margin-top: 1.25rem; font-size: .8125rem; color: #6b7280; }
    code {
      display: block;
      margin-top: 1.25rem;
      padding: .75rem .875rem;
      text-align: left;
      font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace;
      color: #fca5a5;
      background: rgba(248,113,113,.08);
      border: 1px solid rgba(248,113,113,.2);
      border-radius: 8px;
      overflow-wrap: anywhere;
    }
    @media (prefers-color-scheme: light) {
      body { background: #f6f7f9; color: #121620; }
      .card { background: #fff; border-color: #e4e7ee; }
      p { color: #5b6474; }
      .app { color: #121620; }
      .hint { color: #8b93a3; }
    }"#;

/// The check icon. **No `xmlns` attribute** — that omission is what makes the page contain zero
/// `https?://` occurrences, which the upstream test asserts.
const CHECK_ICON: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>"#;

/// The cross icon; same `xmlns`-free rule as [`CHECK_ICON`].
const CROSS_ICON: &str = r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 6 6 18M6 6l12 12"/></svg>"#;

/// `page(options)` (`mcp-callback-server.ts:91`) — the shared template, byte for byte.
fn page(
    title: &str,
    heading: &str,
    body: &str,
    icon: &str,
    tone: &str,
    extra: Option<&str>,
    auto_close: bool,
) -> String {
    let extra = extra.unwrap_or("");
    let script = if auto_close {
        "  <script>setTimeout(() => window.close(), 2000);</script>\n"
    } else {
        ""
    };
    format!(
        "<!DOCTYPE html>\n\
<html lang=\"en\">\n\
<head>\n  \
<meta charset=\"utf-8\">\n  \
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n  \
<title>{title}</title>\n  \
<style>{PAGE_STYLE}\n  </style>\n\
</head>\n\
<body>\n  \
<main class=\"card\">\n    \
<div class=\"badge {tone}\">{icon}</div>\n    \
<h1>{heading}</h1>\n    \
<p>{body}</p>\n    \
{extra}\n  \
</main>\n\
{script}</body>\n\
</html>"
    )
}

/// `htmlSuccess()` — the loopback callback landed and the flow is completing. Auto-closes after
/// 2000 ms. The title separator is an em dash, U+2014, not a hyphen.
#[must_use]
pub fn html_success() -> String {
    let app = escape_html(&app_name());
    page(
        &format!("{app} — Authorization Successful"),
        "Authorization Successful",
        &format!(r#"You can close this window and return to <span class="app">{app}</span>."#),
        CHECK_ICON,
        "ok",
        None,
        true,
    )
}

/// `htmlManualSuccess()` — branch 7's page: the state was *reserved* but nothing is awaiting it, so
/// the user has to carry the URL back themselves. No auto-close, because the address bar is the
/// payload.
#[must_use]
pub fn html_manual_success() -> String {
    let app = escape_html(&app_name());
    page(
        &format!("{app} — Authorization Received"),
        "Authorization Received",
        &format!(
            r#"Copy the full callback URL from your browser address bar and paste it back into <span class="app">{app}</span> with auth-complete."#
        ),
        CHECK_ICON,
        "ok",
        None,
        false,
    )
}

/// `htmlError(error)` — the provider-controlled text is escaped **in the served HTML** while the
/// rejection message carries the raw original; both halves are asserted upstream.
#[must_use]
pub fn html_error(error: &str) -> String {
    let app = escape_html(&app_name());
    page(
        &format!("{app} — Authorization Failed"),
        "Authorization Failed",
        &format!(
            r#"Something went wrong during authorization. You can close this window and try again from <span class="app">{app}</span>."#
        ),
        CROSS_ICON,
        "bad",
        Some(&format!("<code>{}</code>", escape_html(error))),
        false,
    )
}

// ===================================================================================================
// 5 · Callback endpoint configuration and `MCP_OAUTH_CALLBACK_PORT` (MCP-304)
// ===================================================================================================

/// `DEFAULT_OAUTH_CALLBACK_PORT` (`mcp-oauth-provider.ts:73`).
///
/// Reached **only** when `oauth.clientId` is set without an `oauth.redirectUri`: a dynamically
/// registered client always binds port `0`.
pub const DEFAULT_OAUTH_CALLBACK_PORT: u16 = 19876;
/// `DEFAULT_OAUTH_CALLBACK_PATH` (`mcp-oauth-provider.ts:74`).
pub const DEFAULT_OAUTH_CALLBACK_PATH: &str = "/callback";
/// `DEFAULT_OAUTH_CALLBACK_HOST` (`mcp-callback-server.ts:199`) — the **advertised** host.
///
/// MCP-339, option (c): bind `127.0.0.1` and advertise `localhost` through the reused listener's
/// `advertise_host` split. Node's `listen(port, "localhost")` binds whatever the resolver returns
/// first and a browser navigating to `http://localhost:<port>` resolves the same way, so the two
/// agree by construction; binding `127.0.0.1` while advertising `localhost` is correct on every
/// host that resolves `localhost` to IPv4. **Named residual:** on a machine where `localhost`
/// resolves to `::1` only, the browser cannot reach the listener — option (a), binding every
/// resolved address, is the fix if a report arrives.
pub const DEFAULT_OAUTH_CALLBACK_HOST: &str = "localhost";
/// What `localhost` is actually bound as. See [`DEFAULT_OAUTH_CALLBACK_HOST`].
const LOOPBACK_BIND_HOST: &str = "127.0.0.1";

/// `CYRUP_MCP_OAUTH_CALLBACK_PORT`, then `MCP_OAUTH_CALLBACK_PORT` — the workspace's standing
/// rename convention for pi's env names (`cyrup_provider::auth::oauth::callback::callback_host`
/// applies the identical two-name ladder).
pub const CALLBACK_PORT_VARS: [&str; 2] = [
    "CYRUP_MCP_OAUTH_CALLBACK_PORT",
    "MCP_OAUTH_CALLBACK_PORT",
];

/// `getConfiguredOAuthCallbackPort()` (`mcp-oauth-provider.ts:88`).
///
/// Upstream reads the environment **once at module load**, accepting the value only when it parses
/// as an integer in `1..=65535` and otherwise ignoring it silently. A `OnceLock` is the
/// load-time-once equivalent without a static initializer: the first caller wins for the life of
/// the process, so a test that mutates the environment afterwards sees the same value the flow
/// does — which is exactly upstream's semantics.
#[must_use]
pub fn configured_callback_port() -> u16 {
    static CONFIGURED: OnceLock<u16> = OnceLock::new();
    *CONFIGURED.get_or_init(|| {
        for name in CALLBACK_PORT_VARS {
            let Ok(raw) = std::env::var(name) else {
                continue;
            };
            if raw.is_empty() {
                continue;
            }
            // `Number.parseInt` + `Number.isInteger && > 0 && <= 65535`. A `u16` parse rejects
            // 65536 and every negative by construction; the explicit `!= 0` covers `"0"`.
            if let Ok(port) = raw.trim().parse::<u16>()
                && port != 0
            {
                return port;
            }
        }
        DEFAULT_OAUTH_CALLBACK_PORT
    })
}

// ===================================================================================================
// 6 · The loopback callback listener (MCP-305, MCP-306, MCP-308, MCP-336)
// ===================================================================================================

/// `OAuthCallbackResult` (`mcp-callback-server.ts:166`) — the authorization code and, when the
/// authorization server sends one, the RFC 9207 `iss` parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationCodeInput {
    /// The `code` query parameter.
    pub code: String,
    /// The `iss` query parameter, present only when the server sent one.
    pub iss: Option<String>,
}

/// `CALLBACK_TIMEOUT_MS` (`mcp-callback-server.ts:188`) — the per-waiter deadline.
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// `"OAuth callback server stopped"`, the message every queued-or-pending caller sees.
const CALLBACK_STOPPED: &str = "OAuth callback server stopped";

/// One waiter. `waitForCallback` promotes a *reservation* into one of these.
struct PendingCallback {
    sender: oneshot::Sender<McpResult<AuthorizationCodeInput>>,
}

#[derive(Default)]
struct CallbackMapsInner {
    /// `pendingAuths` — a state with someone awaiting it.
    pending: HashMap<String, PendingCallback>,
    /// `reservedAuthStates` — a state the flow has claimed but is not yet awaiting. This is the
    /// headless path: `startAuth` reserves before the browser opens, and `waitForCallback`
    /// promotes it later, from `authenticate`.
    reserved: HashSet<String>,
}

/// The two maps the request handler routes on. Shared by `Arc` with every listener instance so a
/// rebind never loses a reservation.
#[derive(Default)]
struct CallbackMaps {
    inner: StdMutex<CallbackMapsInner>,
}

impl CallbackMaps {
    /// `pendingAuths.size > 0 || reservedAuthStates.size > 0` — the interlock that refuses an
    /// endpoint or path switch.
    fn busy(&self) -> bool {
        self.inner
            .lock()
            .map(|maps| !maps.pending.is_empty() || !maps.reserved.is_empty())
            .unwrap_or(false)
    }

    fn reserve(&self, state: &str) {
        if let Ok(mut maps) = self.inner.lock() {
            maps.reserved.insert(state.to_string());
        }
    }

    fn release(&self, state: &str) {
        if let Ok(mut maps) = self.inner.lock() {
            maps.reserved.remove(state);
        }
    }

    fn pending_count(&self) -> usize {
        self.inner.lock().map(|maps| maps.pending.len()).unwrap_or(0)
    }
}

/// The multi-tenant request handler — MCP-306, the section's one `critical`.
///
/// **The CSRF boundary.** An attacker who can drive the user's browser to
/// `http://localhost:<port>/callback` must not be able to inject an authorization code into a flow
/// (no `state` ⇒ 400; unknown `state` ⇒ 400) and must not be able to reflect arbitrary text into
/// the page for a `state` this process does not recognise (branch 3 deliberately suppresses the
/// provider's `error_description`).
///
/// **There is no HTTP method check** — a `POST` to the callback path is handled identically to a
/// `GET`. Upstream has none; do not add one.
///
/// This handler never calls [`CallbackControl::claim`] and always returns
/// [`CallbackOutcome::Continue`], so the reused listener's own one-shot is never settled, its 409
/// "already used" branch is unreachable, and one listener serves N concurrent logins.
#[derive(Clone)]
struct CallbackMultiplexer {
    maps: Arc<CallbackMaps>,
}

#[async_trait::async_trait]
impl cyrup_provider::auth::oauth::callback::CallbackHandler for CallbackMultiplexer {
    type Value = ();

    async fn handle(
        &self,
        request: cyrup_provider::auth::oauth::callback::CallbackRequest,
        _control: cyrup_provider::auth::oauth::callback::CallbackControl,
    ) -> cyrup_provider::auth::oauth::callback::CallbackOutcome<()> {
        use cyrup_provider::auth::oauth::callback::{CallbackOutcome, CallbackReply};

        // Branch 1 (a foreign path) is answered by the reused listener's `serve_connection`
        // before this runs — with the HTML 404 page rather than upstream's `text/plain`
        // `Not found`. Named delta; cosmetic.
        let code = request.param("code").map(str::to_string);
        let iss = request.param("iss").map(str::to_string);
        let state = request.param("state").map(str::to_string);
        let error = request.param("error").map(str::to_string);
        let error_description = request.param("error_description").map(str::to_string);

        // Branch 2 — CSRF: a callback with no `state` belongs to no flow.
        let Some(state) = state else {
            return CallbackOutcome::Continue {
                reply: CallbackReply::new(
                    400,
                    html_error("Missing required state parameter - potential CSRF attack"),
                ),
            };
        };

        let (is_pending, is_reserved) = {
            match self.maps.inner.lock() {
                Ok(maps) => (
                    maps.pending.contains_key(&state),
                    maps.reserved.contains(&state),
                ),
                Err(_) => (false, false),
            }
        };

        if let Some(error) = error {
            // Branch 3 — an `error` for a state we do not know. The provider's
            // `error_description` is deliberately NOT reflected: an attacker who can drive the
            // browser here must not be able to paint arbitrary text into a page that looks like
            // ours.
            if !is_pending && !is_reserved {
                return CallbackOutcome::Continue {
                    reply: CallbackReply::new(
                        400,
                        html_error("Invalid or expired state parameter - potential CSRF attack"),
                    ),
                };
            }

            // Branch 4 — a known state. The map effects are conditional on the state being
            // *pending*: a reserved-but-not-awaited state keeps its reservation, so a user who
            // gets `?error=access_denied` on the headless path can retry the same state and still
            // be served by branch 7.
            let message = error_description.unwrap_or(error);
            if is_pending
                && let Ok(mut maps) = self.maps.inner.lock()
            {
                maps.reserved.remove(&state);
                if let Some(pending) = maps.pending.remove(&state) {
                    // The rejection carries the RAW provider text; only the served HTML is
                    // escaped. Upstream defers this by one tick so the response flushes first;
                    // here the reply is written by `serve_connection` *after* this handler
                    // returns and nothing on the flow side can close the socket in between, so
                    // the page is delivered either way and the deferral has no work to do.
                    let _ = pending.sender.send(Err(McpError::other(message.clone())));
                }
            }
            return CallbackOutcome::Continue {
                reply: CallbackReply::new(200, html_error(&message)),
            };
        }

        // Branch 5 — an unknown state with no `error`.
        if !is_pending && !is_reserved {
            return CallbackOutcome::Continue {
                reply: CallbackReply::new(
                    400,
                    html_error("Invalid or expired state parameter - potential CSRF attack"),
                ),
            };
        }

        // Branch 6 — no code. The waiter is deliberately **left pending**: this is a malformed
        // callback, not a failed authorization, and the real one may still arrive.
        let Some(code) = code else {
            return CallbackOutcome::Continue {
                reply: CallbackReply::new(400, html_error("No authorization code provided")),
            };
        };

        // Branch 7 — reserved but not yet awaited: the headless path. Nothing is removed and the
        // reservation survives.
        if !is_pending {
            return CallbackOutcome::Continue {
                reply: CallbackReply::new(200, html_manual_success()),
            };
        }

        // Branch 8 — the pending waiter wins.
        if let Ok(mut maps) = self.maps.inner.lock()
            && let Some(pending) = maps.pending.remove(&state)
        {
            let _ = pending.sender.send(Ok(AuthorizationCodeInput { code, iss }));
        }
        CallbackOutcome::Continue {
            reply: CallbackReply::new(200, html_success()),
        }
    }
}

/// The live listener plus the three values a rebind compares against.
struct ListenerSlot {
    server: Option<cyrup_provider::auth::oauth::callback::CallbackServer<()>>,
    /// The **requested/advertised** host — `callbackServerHost`.
    host: String,
    /// The live path — `oauthCallbackPath`.
    path: String,
    /// The live port — `oauthCallbackPort`.
    port: u16,
}

/// The process-shared listener. Upstream this is four module-level `let`s in
/// `mcp-callback-server.ts`; the shape is identical, the mutability is explicit.
struct CallbackRuntime {
    maps: Arc<CallbackMaps>,
    /// The serializing wrapper — upstream's `bindingPromise`. Held across the bind.
    listener: AsyncMutex<ListenerSlot>,
    /// `callbackGeneration`. Compared against a value captured **before** the lock is taken, which
    /// a cancel token cannot express.
    generation: AtomicU64,
    /// `stoppingPromise` — a single shared stop future that is **taken and cleared** when it
    /// completes, so the listener is restartable. A `OnceCell` here would wedge it permanently.
    stopping: StdMutex<Option<Shared<BoxFuture<'static, ()>>>>,
}

fn callback_runtime() -> &'static Arc<CallbackRuntime> {
    static RUNTIME: OnceLock<Arc<CallbackRuntime>> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        Arc::new(CallbackRuntime {
            maps: Arc::new(CallbackMaps::default()),
            listener: AsyncMutex::new(ListenerSlot {
                server: None,
                host: DEFAULT_OAUTH_CALLBACK_HOST.to_string(),
                path: DEFAULT_OAUTH_CALLBACK_PATH.to_string(),
                port: configured_callback_port(),
            }),
            generation: AtomicU64::new(0),
            stopping: StdMutex::new(None),
        })
    })
}

/// `EnsureCallbackServerOptions` (`mcp-callback-server.ts:190`).
#[derive(Debug, Clone, Default)]
pub struct EnsureCallbackServer {
    /// `Boolean(config.clientId) || config.redirectUri !== undefined`. When false the listener
    /// binds port `0`.
    pub strict_port: bool,
    /// The port a strict bind requires; defaults to [`configured_callback_port`].
    pub port: Option<u16>,
    /// The host to advertise (and, after the `localhost`→`127.0.0.1` mapping, to bind).
    pub callback_host: Option<String>,
    /// The path to serve.
    pub callback_path: Option<String>,
    /// The flow's `state` nonce.
    pub oauth_state: Option<String>,
    /// Reserve `oauth_state` **inside the same critical section as the bind**, so a subsequent
    /// host or path switch is refused by it.
    pub reserve_state: bool,
}

fn stopped_error() -> McpError {
    McpError::other(CALLBACK_STOPPED)
}

/// `ensureCallbackServer(options)` (`mcp-callback-server.ts:290`) — MCP-305.
///
/// The serializing wrapper: refuse outright while a stop is in flight, snapshot the generation,
/// take the bind lock (upstream awaits `bindingPromise`), re-check the generation, then run the
/// locked routine.
///
/// **Bind-then-advertise is load-bearing.** `startAuth` calls this *before* the authorization
/// request is built, so the redirect URI handed to the authorization server is the port the
/// listener actually bound. Reverse the two and every dynamically-registered client advertises
/// `http://localhost:19876/callback` while the listener sits on an OS-assigned port — a permanent,
/// provider-side, on-disk consequence.
pub async fn ensure_callback_server(options: EnsureCallbackServer) -> McpResult<()> {
    let runtime = callback_runtime();
    if runtime
        .stopping
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .is_some()
    {
        return Err(stopped_error());
    }
    let generation = runtime.generation.load(Ordering::SeqCst);
    let mut slot = runtime.listener.lock().await;
    if generation != runtime.generation.load(Ordering::SeqCst) {
        return Err(stopped_error());
    }
    ensure_callback_server_locked(runtime, &mut slot, options).await
}

async fn ensure_callback_server_locked(
    runtime: &Arc<CallbackRuntime>,
    slot: &mut ListenerSlot,
    options: EnsureCallbackServer,
) -> McpResult<()> {
    use cyrup_provider::auth::oauth::OAuthError;
    use cyrup_provider::auth::oauth::callback::{CallbackServer, CallbackServerConfig};

    let required_port = options.port.unwrap_or_else(configured_callback_port);
    let strict_port = options.strict_port;
    let requested_host = options
        .callback_host
        .clone()
        .unwrap_or_else(|| DEFAULT_OAUTH_CALLBACK_HOST.to_string());
    let raw_path = options
        .callback_path
        .clone()
        .unwrap_or_else(|| DEFAULT_OAUTH_CALLBACK_PATH.to_string());
    let requested_path = if raw_path.starts_with('/') {
        raw_path
    } else {
        format!("/{raw_path}")
    };
    if options.reserve_state && options.oauth_state.is_none() {
        return Err(McpError::other(
            "OAuth callback reservation requires an oauthState",
        ));
    }

    let has_previous = slot.server.is_some();
    let needs_strict_rebind = has_previous && strict_port && slot.port != required_port;
    let needs_host_switch = has_previous && slot.host != requested_host;
    let needs_path_switch = has_previous && slot.path != requested_path;

    if has_previous {
        if !needs_strict_rebind && !needs_host_switch && !needs_path_switch {
            // Reuse the listener untouched.
            if let Some(state) = options.oauth_state.as_deref()
                && options.reserve_state
            {
                runtime.maps.reserve(state);
            }
            return Ok(());
        }

        if !needs_strict_rebind && !needs_host_switch {
            // Path-only switch.
            if runtime.maps.busy() {
                return Err(McpError::other(format!(
                    "OAuth callback server is using path {}, but callback path {requested_path} is required and cannot be switched while authorizations are pending",
                    slot.path
                )));
            }
            // **Named delta.** Upstream switches the path in place because `handleRequest` reads
            // the live value on every request; the reused listener captures its path at `start`,
            // so the port rebinds instead. It is only reachable with no pending or reserved
            // authorizations (the check above), and in practice only with `strict_port` set —
            // a custom path arrives with an `oauth.redirectUri`, which forces strict.
        } else if runtime.maps.busy() {
            return Err(McpError::other(format!(
                "OAuth callback server is running on {}:{}, but strict callback endpoint {requested_host}:{required_port} is required and cannot be switched while authorizations are pending",
                slot.host, slot.port
            )));
        }
    }

    let listen_port = if strict_port { required_port } else { 0 };
    let bind_host = if requested_host == DEFAULT_OAUTH_CALLBACK_HOST {
        LOOPBACK_BIND_HOST.to_string()
    } else {
        requested_host.clone()
    };
    let config = CallbackServerConfig {
        host: bind_host,
        advertise_host: Some(requested_host.clone()),
        port: listen_port,
        path: requested_path.clone(),
        timeout: None,
        timeout_message: None,
        cancel: None,
    };

    let handler = CallbackMultiplexer {
        maps: Arc::clone(&runtime.maps),
    };
    let candidate = match CallbackServer::start(config, handler).await {
        Ok(server) => server,
        Err(error) => {
            // Nothing was reserved on this path — upstream reserves only *after* a successful
            // listen — so the catch's `releaseCallbackServer` has no work here. The one rewrite
            // that matters is the strict-port collision.
            if strict_port
                && let OAuthError::Listen { source, .. } = &error
                && source.kind() == std::io::ErrorKind::AddrInUse
            {
                return Err(McpError::other(format!(
                    "OAuth callback port {required_port} is already in use. Pre-registered OAuth clients require an exact redirect URI; set {} to your registered port or free port {required_port}",
                    CALLBACK_PORT_VARS[0]
                )));
            }
            return Err(McpError::other(error.to_string()));
        }
    };

    // A new listener is bound **first**; only then is the old one closed. A failed rebind must
    // leave the existing listener serving.
    let assigned = candidate.port();
    if let Some(previous) = slot.server.take() {
        previous.close();
    }
    slot.server = Some(candidate);
    slot.host = requested_host;
    slot.path = requested_path;
    slot.port = assigned;

    if let Some(state) = options.oauth_state.as_deref()
        && options.reserve_state
    {
        runtime.maps.reserve(state);
    }
    Ok(())
}

/// The redirect URI the listener is currently advertising — read **after**
/// [`ensure_callback_server`], never before (bind-then-advertise).
///
/// `None` when nothing is bound.
#[must_use]
pub async fn callback_redirect_uri() -> Option<String> {
    let runtime = callback_runtime();
    let slot = runtime.listener.lock().await;
    slot.server
        .as_ref()
        .map(|server| server.redirect_uri().to_string())
}

/// The live `(port, path)` pair — `getOAuthCallbackPort()` / `getOAuthCallbackPath()`.
#[must_use]
pub async fn callback_endpoint() -> (u16, String) {
    let runtime = callback_runtime();
    let slot = runtime.listener.lock().await;
    (slot.port, slot.path.clone())
}

/// `reserveCallbackServer(oauthState)` (`mcp-callback-server.ts:414`).
pub fn reserve_callback_server(oauth_state: &str) {
    callback_runtime().maps.reserve(oauth_state);
}

/// `releaseCallbackServer(oauthState)` (`mcp-callback-server.ts:418`).
pub fn release_callback_server(oauth_state: &str) {
    callback_runtime().maps.release(oauth_state);
}

/// `waitForCallback(oauthState)` (`mcp-callback-server.ts:427`).
///
/// Promotes a *reservation* into a pending wait — the reservation is deleted first — and arms the
/// [`CALLBACK_TIMEOUT`] deadline, which fires **only if the state is still pending**.
///
/// The returned receiver is the waiter; register it **before** the browser opens.
///
/// **Named delta.** Upstream's callback timer is deliberately *not* unref'd (it holds the Node
/// event loop open) where the flow-side abandon timer is. Here both are detached tokio tasks and
/// neither holds the process open, which is the stronger of the two guarantees and the one
/// MCP-308's process-exit test asserts.
pub fn wait_for_callback(oauth_state: &str) -> oneshot::Receiver<McpResult<AuthorizationCodeInput>> {
    let runtime = callback_runtime();
    let (sender, receiver) = oneshot::channel();
    if let Ok(mut maps) = runtime.maps.inner.lock() {
        maps.reserved.remove(oauth_state);
        maps.pending
            .insert(oauth_state.to_string(), PendingCallback { sender });
    }

    let state = oauth_state.to_string();
    let maps = Arc::clone(&runtime.maps);
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(async move {
            tokio::time::sleep(CALLBACK_TIMEOUT).await;
            // The map lookup *is* the cancellation: a state that completed or was cancelled is
            // no longer pending and this task does nothing.
            if let Ok(mut maps) = maps.inner.lock()
                && let Some(pending) = maps.pending.remove(&state)
            {
                let _ = pending.sender.send(Err(McpError::other(
                    "OAuth callback timeout - authorization took too long",
                )));
            }
        });
    }
    receiver
}

/// `cancelPendingCallback(oauthState)` (`mcp-callback-server.ts:444`).
pub fn cancel_pending_callback(oauth_state: &str) {
    let runtime = callback_runtime();
    if let Ok(mut maps) = runtime.maps.inner.lock() {
        maps.reserved.remove(oauth_state);
        if let Some(pending) = maps.pending.remove(oauth_state) {
            let _ = pending
                .sender
                .send(Err(McpError::other("Authorization cancelled")));
        }
    }
}

/// `stopCallbackServer()` (`mcp-callback-server.ts:457`) — MCP-308.
///
/// Bumps the generation, drains any in-flight bind, closes and clears the listener, resets the live
/// port/host/path to their configured defaults, takes both maps, and settles every waiter with
/// `"OAuth callback server stopped"`.
///
/// Idempotent while in flight through a single shared future, and **that future is cleared when it
/// completes, so the listener is restartable**: only calls queued before or during the stop are
/// rejected, by the generation check.
pub async fn stop_callback_server() {
    let runtime = callback_runtime();

    // Build the shared future while holding the std lock — creating it is synchronous, so two
    // concurrent callers cannot both decide they are the owner.
    let (future, is_owner) = {
        let Ok(mut guard) = runtime.stopping.lock() else {
            return;
        };
        match guard.as_ref() {
            Some(existing) => (existing.clone(), false),
            None => {
                runtime.generation.fetch_add(1, Ordering::SeqCst);
                let runtime = Arc::clone(runtime);
                let future = async move { stop_callback_server_inner(&runtime).await }
                    .boxed()
                    .shared();
                *guard = Some(future.clone());
                (future, true)
            }
        }
    };

    future.await;

    if is_owner
        && let Ok(mut guard) = runtime.stopping.lock()
    {
        *guard = None;
    }
}

async fn stop_callback_server_inner(runtime: &Arc<CallbackRuntime>) {
    // Taking the bind lock *is* upstream's `while (bindingPromise) await bindingPromise.catch()`.
    {
        let mut slot = runtime.listener.lock().await;
        if let Some(server) = slot.server.take() {
            server.close();
        }
        slot.port = configured_callback_port();
        slot.host = DEFAULT_OAUTH_CALLBACK_HOST.to_string();
        slot.path = DEFAULT_OAUTH_CALLBACK_PATH.to_string();
    }

    // Snapshot and clear both maps before settling, so a caller awaiting the stop observes the
    // reset state before the rejections land (upstream's deferred tick).
    let drained = {
        match runtime.maps.inner.lock() {
            Ok(mut maps) => {
                maps.reserved.clear();
                maps.pending.drain().map(|(_, p)| p).collect::<Vec<_>>()
            }
            Err(_) => Vec::new(),
        }
    };
    for pending in drained {
        let _ = pending.sender.send(Err(stopped_error()));
    }
}

/// `isCallbackServerRunning()` (`mcp-callback-server.ts:501`).
#[must_use]
pub async fn is_callback_server_running() -> bool {
    callback_runtime().listener.lock().await.server.is_some()
}

/// `getPendingAuthCount()` (`mcp-callback-server.ts:508`).
#[must_use]
pub fn pending_callback_count() -> usize {
    callback_runtime().maps.pending_count()
}

// ===================================================================================================
// 7 · The storage seam this flow consumes (MCP-314, MCP-315, MCP-321)
// ===================================================================================================

/// The registration record and the stored-entry shape are **section 13f's**
/// ([`crate::credentials`]), re-exported rather than duplicated.
///
/// [`crate::credentials::AuthEntry`] is `{ credentials, client, state, server_url }` and
/// [`crate::credentials::StoredClientInfo`] carries the dynamic-client-registration fields rmcp's
/// [`StoredCredentials`] drops — `client_secret`, `client_id_issued_at`,
/// `client_secret_expires_at`, `redirect_uris`, `issuer` and the `config_pre_registered` stub
/// marker (MCP-290).
///
/// **Why this matters to the flow (MCP-314).** `rmcp` persists **only** `client_id`, and
/// `initialize_from_store` re-applies it through `configure_client_id`, which additionally resets
/// `redirect_uri` to the manager's base URL. A confidential or explicitly-redirected client is
/// therefore mis-configured after a restart unless the port re-applies the rest —
/// [`restore_client_configuration`] is that re-apply, and it reads this record.
///
/// The PKCE/CSRF slot on `AuthEntry` is deliberately **not** read here: the flow reaches that
/// material through rmcp's `StateStore`, never through the entry (MCP-320).
pub use crate::credentials::{AuthEntry, StoredClientInfo};

/// `clientInfo.clientSecretExpiresAt && clientInfo.clientSecretExpiresAt < Date.now() / 1000`
/// (`mcp-oauth-provider.ts:313`) — a stored secret past its expiry forces re-registration.
///
/// `0` means "never expires" per RFC 7591 §3.2.1, which upstream's bare `<` comparison gets wrong
/// (a zero timestamp is always in the past); the RFC is followed here and the divergence recorded
/// rather than inherited.
#[must_use]
pub fn client_secret_expired(record: &StoredClientInfo, now_secs: f64) -> bool {
    record
        .client_secret_expires_at
        .is_some_and(|expires| expires != 0.0 && expires < now_secs)
}

/// The storage the OAuth flow needs, as a trait so section 13f's keyring store and this flow can
/// land independently.
///
/// Every method is keyed by server name because the keychain account is derived from it
/// (`sha256-<hex>` of the name); the `server_url` arguments are the URL binding, not a key.
///
/// The production implementation is [`crate::credentials::McpAuthStore`] (section 13f owns it,
/// including the keychain, the chunking manifest, the process-lifetime cache and the `keyctl`
/// recovery). [`InMemoryOAuthStorage`] is the test double.
#[async_trait::async_trait]
pub trait McpOAuthStorage: Send + Sync + 'static {
    /// `getAuthEntry(serverName)` — the **unvalidated** read. Mutators use this; the flow uses
    /// [`Self::get_auth_for_url`].
    async fn load(&self, server_name: &str) -> McpResult<Option<AuthEntry>>;

    /// `updateTokens` / `clearTokens`. `None` clears. When `server_url` differs from the stored
    /// one, the implementation purges the entry's other artifacts (MCP-264).
    async fn save_credentials(
        &self,
        server_name: &str,
        server_url: &str,
        credentials: Option<StoredCredentials>,
    ) -> McpResult<()>;

    /// `updateClientInfo` / `clearClientInfo`, with the same URL-binding rule.
    async fn save_client(
        &self,
        server_name: &str,
        server_url: &str,
        client: Option<StoredClientInfo>,
    ) -> McpResult<()>;

    /// `clearAllCredentials(serverName)` — `removeAuthEntry`.
    async fn clear_all(&self, server_name: &str) -> McpResult<()>;

    /// `getOAuthState(serverName, options)` (`mcp-auth.ts:938`) — the persisted CSRF nonce, or
    /// `None`.
    ///
    /// Under MCP-320 the *live* flow keeps its PKCE/CSRF material in rmcp's `InMemoryStateStore`
    /// and never writes this slot, so on a healthy run this reads `None`. It exists because
    /// `clearPendingAuth` (`mcp-auth-flow.ts:494`) reads it: a slot written by an older build, or
    /// by a transport tier that wires 13f's `McpStateStore`, would otherwise never be reclaimed.
    async fn oauth_state(&self, server_name: &str) -> McpResult<Option<String>>;

    /// `clearOAuthState(serverName, options)` (`mcp-auth.ts:946`) — drop the nonce, keep the rest
    /// of the entry. Upstream passes `serverUrl` as `undefined` here, so the URL-binding purge
    /// cannot fire on a clear.
    async fn clear_oauth_state(&self, server_name: &str) -> McpResult<()>;

    /// `getAuthBaseDir(options)`. Folded into the pending-auth and in-flight dedup keys, which is
    /// the only reason the flow needs it (MCP-266).
    fn base_dir(&self) -> PathBuf;

    /// `getAuthForUrl(serverName, serverUrl)` (`mcp-auth.ts`) — the URL-bound read.
    ///
    /// Returns nothing when the stored `serverUrl` is absent or differs. Comparison is **exact
    /// string equality**: no normalization, no trailing-slash tolerance, because adding either
    /// would silently widen credential reuse.
    async fn get_auth_for_url(
        &self,
        server_name: &str,
        server_url: &str,
    ) -> McpResult<Option<AuthEntry>> {
        let Some(entry) = self.load(server_name).await? else {
            return Ok(None);
        };
        match entry.server_url.as_deref() {
            Some(stored) if stored == server_url => Ok(Some(entry)),
            _ => Ok(None),
        }
    }
}

/// The in-process [`McpOAuthStorage`] — the test double, and the interim default until section
/// 13f's keyring store is wired in.
///
/// It reproduces the URL-binding purge rule so tests written against it are not lying about the
/// real store's behaviour.
#[derive(Debug, Default)]
pub struct InMemoryOAuthStorage {
    entries: StdMutex<HashMap<String, AuthEntry>>,
    base_dir: PathBuf,
}

impl InMemoryOAuthStorage {
    /// An empty store rooted at `base_dir` (which only ever reaches a map key).
    #[must_use]
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            entries: StdMutex::new(HashMap::new()),
            base_dir,
        }
    }
}

#[async_trait::async_trait]
impl McpOAuthStorage for InMemoryOAuthStorage {
    async fn load(&self, server_name: &str) -> McpResult<Option<AuthEntry>> {
        Ok(self
            .entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(server_name).cloned()))
    }

    async fn save_credentials(
        &self,
        server_name: &str,
        server_url: &str,
        credentials: Option<StoredCredentials>,
    ) -> McpResult<()> {
        let Ok(mut entries) = self.entries.lock() else {
            return Ok(());
        };
        let entry = entries.entry(server_name.to_string()).or_default();
        if entry.server_url.as_deref().is_some_and(|url| url != server_url) {
            // The sibling-purge rule: a stale client record from a previous authorization server
            // must never be paired with a new one.
            entry.client = None;
        }
        entry.server_url = Some(server_url.to_string());
        entry.credentials = credentials;
        Ok(())
    }

    async fn save_client(
        &self,
        server_name: &str,
        server_url: &str,
        client: Option<StoredClientInfo>,
    ) -> McpResult<()> {
        let Ok(mut entries) = self.entries.lock() else {
            return Ok(());
        };
        let entry = entries.entry(server_name.to_string()).or_default();
        if entry.server_url.as_deref().is_some_and(|url| url != server_url) {
            entry.credentials = None;
        }
        entry.server_url = Some(server_url.to_string());
        entry.client = client;
        Ok(())
    }

    async fn clear_all(&self, server_name: &str) -> McpResult<()> {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(server_name);
        }
        Ok(())
    }

    async fn oauth_state(&self, server_name: &str) -> McpResult<Option<String>> {
        Ok(self.entries.lock().ok().and_then(|entries| {
            entries
                .get(server_name)
                .and_then(|entry| entry.state.as_ref())
                .map(|state| state.csrf_token.clone())
        }))
    }

    async fn clear_oauth_state(&self, server_name: &str) -> McpResult<()> {
        if let Ok(mut entries) = self.entries.lock()
            && let Some(entry) = entries.get_mut(server_name)
        {
            entry.state = None;
        }
        Ok(())
    }

    fn base_dir(&self) -> PathBuf {
        self.base_dir.clone()
    }
}

/// The per-server [`CredentialStore`] rmcp writes tokens through (MCP-315).
///
/// `rmcp::transport::auth::CredentialStore` is `load`/`save`/`clear` with **no key**, so this crate
/// instantiates one per server, bound to that server's account key and URL. That is the natural
/// shape, not a workaround.
///
/// **Overlap, named.** [`crate::credentials::McpCredentialStore`] is the same adapter written
/// against the concrete keychain store; this one is generic over [`McpOAuthStorage`], which is what
/// lets the flow run against [`InMemoryOAuthStorage`] in a test with no keychain. If the trait is
/// ever dissolved, this is the half that goes.
pub struct ServerCredentialStore {
    storage: Arc<dyn McpOAuthStorage>,
    server_name: String,
    server_url: String,
}

impl ServerCredentialStore {
    /// Bind a store to one server's `(name, url)` pair.
    #[must_use]
    pub fn new(storage: Arc<dyn McpOAuthStorage>, server_name: &str, server_url: &str) -> Self {
        Self {
            storage,
            server_name: server_name.to_string(),
            server_url: server_url.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl CredentialStore for ServerCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        self.storage
            .get_auth_for_url(&self.server_name, &self.server_url)
            .await
            .map(|entry| entry.and_then(|entry| entry.credentials))
            .map_err(|error| AuthError::InternalError(error.to_string()))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        self.storage
            .save_credentials(&self.server_name, &self.server_url, Some(credentials))
            .await
            .map_err(|error| AuthError::InternalError(error.to_string()))
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.storage
            .save_credentials(&self.server_name, &self.server_url, None)
            .await
            .map_err(|error| AuthError::InternalError(error.to_string()))
    }
}

/// The token fields this crate reads off rmcp's `OAuthTokenResponse` — upstream `StoredTokens`
/// minus the storage concerns (MCP-315).
///
/// `expires_at` is a **Unix timestamp in seconds** and `token_type` is hardcoded `"Bearer"` on
/// read, both exactly as upstream. The write-side rule that `expiresAt = now + expires_in` **even
/// when `expires_in` is 0** is `rmcp`'s to keep: it stores `token_received_at` and the raw
/// `expires_in`, so an already-expired token stays expired rather than becoming never-expiring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpTokens {
    /// The bearer token.
    pub access_token: String,
    /// The refresh token, when the authorization server issued one. A refresh response that omits
    /// one **keeps the old one** (RFC 6749 §6) — rmcp does this for free.
    pub refresh_token: Option<String>,
    /// Unix seconds. `None` means the server sent no expiry information, which
    /// [`get_valid_token`]'s fall-through reads as "assume valid".
    pub expires_at: Option<i64>,
    /// The granted scope string.
    pub scope: Option<String>,
    /// The issuer binding.
    pub issuer: Option<String>,
}

impl McpTokens {
    /// `token_type` is hardcoded `"Bearer"` on read (`mcp-oauth-provider.ts:396`).
    pub const TOKEN_TYPE: &'static str = "Bearer";
}

/// Project rmcp's stored token response into [`McpTokens`].
///
/// **Named delta / Cargo ask.** `oauth2::TokenResponse`'s getters (`access_token`, `refresh_token`,
/// `expires_in`, `scopes`) are trait methods, and `oauth2` is not a direct dependency of this
/// crate — `rmcp` selects it privately through its `auth` feature and re-exports nothing. The
/// projection therefore goes through the response's `Serialize` impl, which is the RFC 6749 §5.1
/// JSON shape and is stable by specification. Adding `oauth2 = "5"` to `Cargo.toml` would let this
/// use the typed getters; it is a one-line change and is filed in the report.
#[must_use]
pub fn project_tokens(stored: &StoredCredentials) -> Option<McpTokens> {
    let response = stored.token_response.as_ref()?;
    let value = serde_json::to_value(response).ok()?;
    let access_token = value.get("access_token")?.as_str()?.to_string();
    let refresh_token = value
        .get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let expires_at = value
        .get("expires_in")
        .and_then(serde_json::Value::as_u64)
        .and_then(|expires_in| {
            // `expiresAt = now_secs + expires_in`, written even when `expires_in` is 0. rmcp
            // stamps `token_received_at` at save time, so the arithmetic is against that rather
            // than against "now" — which is the same number, computed once instead of on every
            // read.
            stored
                .token_received_at
                .map(|received| received.saturating_add(expires_in) as i64)
        });
    let scope = value
        .get("scope")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            (!stored.granted_scopes.is_empty()).then(|| stored.granted_scopes.join(" "))
        });
    Some(McpTokens {
        access_token,
        refresh_token,
        expires_at,
        scope,
        issuer: stored.issuer.clone(),
    })
}

/// `isTokenExpired(serverName)` (`mcp-auth.ts`) — **tri-state**, and the tri-state is the contract:
///
/// * `None` — there are no tokens;
/// * `Some(false)` — there is no `expiresAt` **or** it lies in the future;
/// * `Some(true)` — otherwise.
///
/// `getAuthStatus` maps a `None` expiry to `"authenticated"` because of the middle row, and
/// [`get_valid_token`]'s fall-through returns the tokens anyway on the same reasoning.
#[must_use]
pub fn is_token_expired(tokens: Option<&McpTokens>, now_secs: i64) -> Option<bool> {
    let tokens = tokens?;
    Some(tokens.expires_at.is_some_and(|expires| expires < now_secs))
}

/// `Date.now() / 1000` — Unix seconds.
#[must_use]
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

// ===================================================================================================
// 8 · Flow ownership: runtime, generation counter, four maps (MCP-301, MCP-329, MCP-344, MCP-345)
// ===================================================================================================

/// `AuthStatus` (`mcp-auth-flow.ts:44`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStatus {
    /// Tokens are stored and not expired — or carry no expiry at all.
    Authenticated,
    /// Tokens are stored and past their expiry.
    Expired,
    /// No tokens are stored.
    NotAuthenticated,
}

impl AuthStatus {
    /// The wire spelling the tool/command layers put in `details.status`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            AuthStatus::Authenticated => "authenticated",
            AuthStatus::Expired => "expired",
            AuthStatus::NotAuthenticated => "not_authenticated",
        }
    }
}

impl std::fmt::Display for AuthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `MANUAL_AUTH_TIMEOUT_MS` (`mcp-auth-flow.ts:137`) — the abandoned-flow deadline (MCP-329).
pub const MANUAL_AUTH_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The reason a runtime shutdown aborts its in-flight logins with.
pub const OAUTH_RUNTIME_STOPPED: &str = "OAuth runtime stopped";

/// The three aggregate phases, plus `completeAuth`'s rethrow-alone case (MCP-345).
///
/// Every one of these surfaces to the user through `Failed to authenticate "<n>": <message>`, so a
/// port that collapsed to the primary error would make the secondary permanently invisible.
pub const PHASE_STARTUP_CLEANUP: &str = "OAuth startup cleanup failed";
/// See [`PHASE_STARTUP_CLEANUP`].
pub const PHASE_COMPLETION_CLEANUP: &str = "OAuth completion cleanup failed";
/// See [`PHASE_STARTUP_CLEANUP`].
pub const PHASE_CANCELLATION_CLEANUP: &str = "OAuth cancellation cleanup failed";

/// `new AggregateError([error, cleanupError], phase)` — MCP-345.
///
/// A credential-store failure during cleanup must not hide the OAuth error that caused it, and
/// vice versa; the rendering is the phase followed by **both** messages, joined the same way
/// [`crate::errors::CleanupErrors`] joins its children — which is `formatTerminalError`'s aggregate
/// walk, deduplicated, so two cleanups failing identically print once.
///
/// **Landed at integration.** This used to flatten into [`McpError::Other`], losing the structure.
/// `errors.rs` now carries [`McpError::OAuthAggregate`], so a caller can match the phase and reach
/// the children — in particular [`McpError::is_credential_store_failure`] walks into them, which is
/// what keeps a broken keychain during cleanup from being read as an ordinary OAuth failure.
#[must_use]
pub fn aggregate_error(phase: &str, primary: McpError, cleanup: McpError) -> McpError {
    // `phase` is always one of the three `PHASE_*` constants; the `&str` parameter is kept so the
    // call sites read as upstream's `new AggregateError(errors, phase)` does.
    let phase: &'static str = match phase {
        PHASE_STARTUP_CLEANUP => PHASE_STARTUP_CLEANUP,
        PHASE_COMPLETION_CLEANUP => PHASE_COMPLETION_CLEANUP,
        PHASE_CANCELLATION_CLEANUP => PHASE_CANCELLATION_CLEANUP,
        other => return McpError::other(format!("{other}: {primary}: {cleanup}")),
    };
    // Owned, never cloned: `clone_error` would flatten a `CredentialStore` into `Other`, and a
    // broken keychain *during cleanup* is exactly the case this aggregate exists to preserve.
    McpError::oauth_aggregate(phase, primary, cleanup)
}

/// Rebuild an error that has to cross a `Shared` future, which requires `Clone`.
///
/// The abort arm is reconstructed faithfully because [`crate::abort::is_abort_error`] matches on
/// it; everything else degrades to [`McpError::Other`] with the identical message.
fn clone_error(error: &McpError) -> McpError {
    match error {
        McpError::Aborted(reason) => McpError::Aborted(reason.clone()),
        McpError::Config(message) => McpError::Config(message.clone()),
        McpError::Server { server, message } => McpError::Server {
            server: server.clone(),
            message: message.clone(),
        },
        other => McpError::Other(other.to_string()),
    }
}

/// One in-flight authorization — upstream's `PendingAuth` (`mcp-auth-flow.ts:72`).
struct PendingAuth {
    server_name: String,
    server_url: String,
    authorization_url: String,
    /// The rmcp session that owns the discovered metadata, the configured client and the PKCE
    /// state. `deactivate()` is `take()`: upstream's provider refuses every call once inactive,
    /// and a `None` here refuses by construction.
    session: Arc<AsyncMutex<Option<AuthorizationSession>>>,
    /// The issuer discovery settled on, for the RFC 9207 gate.
    expected_issuer: Option<String>,
    /// `authorization_response_iss_parameter_supported === true`.
    requires_issuer: bool,
    /// The storage this flow was started against; the **stored** options win over a later caller's
    /// fallback for every subsequent store operation.
    storage: Arc<dyn McpOAuthStorage>,
    /// `getAuthBaseDir(pendingAuth.authStorageOptions)`, folded into the map key.
    base_dir: PathBuf,
}

#[derive(Default)]
struct RuntimeState {
    /// `pendingAuths`, keyed by `` `${serverName}|${authBaseDir}` ``.
    pending_auths: HashMap<String, PendingAuth>,
    /// `pendingAuthStates`, same key → the flow's `state` nonce.
    pending_auth_states: HashMap<String, String>,
    /// `pendingAuthCleanupTimers`, same key → the abandon timer's cancel handle.
    pending_auth_timers: HashMap<String, CancelToken>,
    /// `pendingAuthentications`, keyed by `` `${serverName}|${serverUrl}|${authBaseDir}` `` —
    /// in-flight dedup.
    pending_authentications: HashMap<String, SharedAuthentication>,
}

type SharedAuthentication = Shared<BoxFuture<'static, Result<AuthStatus, Arc<McpError>>>>;

/// One OAuth runtime — upstream `McpOAuthRuntime` (`mcp-auth-flow.ts:46`), which is
/// `{ readonly signal: AbortSignal }` with all mutable state hanging off a `WeakMap`.
///
/// Here the `Arc` **is** the ownership: the state hangs off the runtime itself and the weak map
/// disappears. A session restart aborts every in-flight login for that session and disturbs no
/// other's; the shared listener outlives any single runtime (MCP-344).
pub struct McpOAuthRuntime {
    /// Identity in the live-runtime set. The set must be idempotent — an incrementing counter
    /// would over-count across `getRuntime` calls and never reach zero.
    id: u64,
    /// `combineAbortSignals(callerSignal, controller.signal)`.
    token: CancelToken,
    /// The runtime's own controller, aborted by [`shutdown_oauth`].
    controller: CancelToken,
    /// Bumped by [`shutdown_oauth`]; `set_pending_auth` refuses to publish if it moved.
    ///
    /// This stays an explicit `AtomicU64` rather than collapsing into the cancel token because it
    /// is compared against a value captured **before** an await point, which a token cannot
    /// express.
    generation: AtomicU64,
    /// `signal.reason` — a `CancelToken` carries no payload.
    stop_reason: StdMutex<Option<String>>,
    state: AsyncMutex<RuntimeState>,
}

impl std::fmt::Debug for McpOAuthRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpOAuthRuntime")
            .field("id", &self.id)
            .field("generation", &self.generation.load(Ordering::SeqCst))
            .field("aborted", &self.controller.is_cancelled())
            .finish()
    }
}

impl McpOAuthRuntime {
    /// The runtime's combined abort token — upstream's `runtime.signal`.
    #[must_use]
    pub fn token(&self) -> &CancelToken {
        &self.token
    }

    /// The generation this runtime is on. Capture it **before** an await, compare it after.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// `runtime.signal.throwIfAborted()`, carrying the stop reason.
    pub fn throw_if_aborted(&self) -> McpResult<()> {
        if !self.token.is_cancelled() {
            return Ok(());
        }
        let reason = self
            .stop_reason
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| OAUTH_RUNTIME_STOPPED.to_string());
        Err(McpError::Aborted(reason))
    }
}

/// The process-global live-runtime set — MCP-344.
///
/// `getRuntime` and `initializeOAuth` **add** on every call, so the collection has to be
/// idempotent; the listener is stopped only when it empties. Two sessions each with their own
/// runtime must not tear down each other's listener.
fn live_runtimes() -> &'static StdMutex<HashSet<u64>> {
    static LIVE: OnceLock<StdMutex<HashSet<u64>>> = OnceLock::new();
    LIVE.get_or_init(|| StdMutex::new(HashSet::new()))
}

fn next_runtime_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::SeqCst)
}

/// `createOAuthRuntime(signal?)` (`mcp-auth-flow.ts:91`) — combine the caller's signal with a fresh
/// controller and register the runtime in the live set.
#[must_use]
pub fn create_oauth_runtime(signal: Option<&CancelToken>) -> Arc<McpOAuthRuntime> {
    let controller = CancelToken::new();
    let token = crate::abort::combine(&controller, signal);
    let runtime = Arc::new(McpOAuthRuntime {
        id: next_runtime_id(),
        token,
        controller,
        generation: AtomicU64::new(0),
        stop_reason: StdMutex::new(None),
        state: AsyncMutex::new(RuntimeState::default()),
    });
    if let Ok(mut live) = live_runtimes().lock() {
        live.insert(runtime.id);
    }
    runtime
}

/// The module-level legacy runtime (`mcp-auth-flow.ts:106`).
///
/// Created on first use and **removed from the live set immediately**, so a process that never
/// calls [`create_oauth_runtime`] still has a working default that does not by itself keep the
/// shared listener alive.
fn legacy_runtime() -> Arc<McpOAuthRuntime> {
    static LEGACY: OnceLock<StdMutex<Option<Arc<McpOAuthRuntime>>>> = OnceLock::new();
    let slot = LEGACY.get_or_init(|| StdMutex::new(None));
    let Ok(mut guard) = slot.lock() else {
        // A poisoned lock cannot happen without a panic, which this crate denies; a fresh
        // unregistered runtime is the safe degradation.
        let runtime = create_oauth_runtime(None);
        if let Ok(mut live) = live_runtimes().lock() {
            live.remove(&runtime.id);
        }
        return runtime;
    };
    if let Some(existing) = guard.as_ref()
        && !existing.controller.is_cancelled()
    {
        return Arc::clone(existing);
    }
    let runtime = create_oauth_runtime(None);
    if let Ok(mut live) = live_runtimes().lock() {
        live.remove(&runtime.id);
    }
    *guard = Some(Arc::clone(&runtime));
    runtime
}

/// `getRuntime(options)` (`mcp-auth-flow.ts:109`) — adopt an explicit runtime after
/// `throwIfAborted`, or resurrect the legacy one; either way **add to the live set**.
pub fn get_runtime(runtime: Option<&Arc<McpOAuthRuntime>>) -> McpResult<Arc<McpOAuthRuntime>> {
    let runtime = match runtime {
        Some(runtime) => {
            runtime.throw_if_aborted()?;
            Arc::clone(runtime)
        }
        None => legacy_runtime(),
    };
    if let Ok(mut live) = live_runtimes().lock() {
        live.insert(runtime.id);
    }
    Ok(runtime)
}

/// `initializeOAuth(runtimeOrSignal?)` (`mcp-auth-flow.ts:944`).
///
/// **Binding is lazy**: this never starts the listener; only [`start_auth`] does.
pub async fn initialize_oauth(
    runtime: Option<&Arc<McpOAuthRuntime>>,
    signal: Option<&CancelToken>,
) -> McpResult<Arc<McpOAuthRuntime>> {
    if let Some(runtime) = runtime {
        runtime.throw_if_aborted()?;
        if let Ok(mut live) = live_runtimes().lock() {
            live.insert(runtime.id);
        }
        return Ok(Arc::clone(runtime));
    }
    let legacy = legacy_runtime();
    shutdown_oauth(&legacy).await;
    Ok(create_oauth_runtime(signal))
}

/// `shutdownOAuth(runtime)` (`mcp-auth-flow.ts:963`) — in exactly this order.
///
/// Returns early if already aborted; increments the generation; aborts the controller with
/// `"OAuth runtime stopped"`; cancels every recorded callback state; clears every pending auth;
/// clears the dedup map; removes the runtime from the live set; **and only if the set is now empty,
/// stops the callback listener** (MCP-344).
pub async fn shutdown_oauth(runtime: &Arc<McpOAuthRuntime>) {
    if runtime.controller.is_cancelled() {
        return;
    }
    runtime.generation.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut reason) = runtime.stop_reason.lock() {
        *reason = Some(OAUTH_RUNTIME_STOPPED.to_string());
    }
    runtime.controller.cancel();

    let (states, pending): (Vec<String>, Vec<(String, PathBuf)>) = {
        let state = runtime.state.lock().await;
        (
            state.pending_auth_states.values().cloned().collect(),
            state
                .pending_auths
                .values()
                .map(|pending| (pending.server_name.clone(), pending.base_dir.clone()))
                .collect(),
        )
    };
    for callback_state in states {
        cancel_pending_callback(&callback_state);
    }
    for (server_name, base_dir) in pending {
        let _ = clear_pending_auth(runtime, &server_name, None, &base_dir, None).await;
    }
    {
        let mut state = runtime.state.lock().await;
        state.pending_authentications.clear();
    }

    let empty = {
        match live_runtimes().lock() {
            Ok(mut live) => {
                live.remove(&runtime.id);
                live.is_empty()
            }
            Err(_) => false,
        }
    };
    if empty {
        stop_callback_server().await;
    }
}

/// `` `${serverName}|${getAuthBaseDir(options)}` `` (`mcp-auth-flow.ts:127`).
fn pending_auth_key(server_name: &str, base_dir: &Path) -> String {
    format!("{server_name}|{}", base_dir.display())
}

/// `hasPendingAuth(serverName, options?, runtime?)` (`mcp-auth-flow.ts:130`).
///
/// Exact key lookup with a base directory, a linear scan without. Its only consumer is the
/// lifecycle manager's idle-shutdown suppression: a server with an OAuth flow in flight must not be
/// reaped.
pub async fn has_pending_auth(
    runtime: &Arc<McpOAuthRuntime>,
    server_name: &str,
    base_dir: Option<&Path>,
) -> bool {
    let state = runtime.state.lock().await;
    match base_dir {
        Some(base_dir) => state
            .pending_auths
            .contains_key(&pending_auth_key(server_name, base_dir)),
        None => state
            .pending_auths
            .values()
            .any(|pending| pending.server_name == server_name),
    }
}

/// `setPendingAuth(...)` (`mcp-auth-flow.ts:449`).
///
/// Clears any prior pending auth for the key, re-checks the abort **and the generation captured
/// before the await**, publishes into both maps, and arms the [`MANUAL_AUTH_TIMEOUT`] abandon timer
/// (MCP-329). The timer is a detached task; nothing waits on it.
async fn set_pending_auth(
    runtime: &Arc<McpOAuthRuntime>,
    pending: PendingAuth,
    oauth_state: &str,
    generation: u64,
) -> McpResult<()> {
    let server_name = pending.server_name.clone();
    let base_dir = pending.base_dir.clone();
    let storage = Arc::clone(&pending.storage);
    let key = pending_auth_key(&server_name, &base_dir);

    // `clearPendingAuth(runtime, serverName, undefined, pendingAuth.authStorageOptions)`
    // (`mcp-auth-flow.ts:460`) — the *incoming* flow's storage is the fallback.
    clear_pending_auth(runtime, &server_name, None, &base_dir, Some(&storage)).await?;
    runtime.throw_if_aborted()?;
    if generation != runtime.generation() {
        return Err(McpError::other(OAUTH_RUNTIME_STOPPED));
    }

    let timer = CancelToken::new();
    {
        let mut state = runtime.state.lock().await;
        state.pending_auths.insert(key.clone(), pending);
        state
            .pending_auth_states
            .insert(key.clone(), oauth_state.to_string());
        state.pending_auth_timers.insert(key, timer.clone());
    }

    if tokio::runtime::Handle::try_current().is_ok() {
        let runtime = Arc::clone(runtime);
        let oauth_state = oauth_state.to_string();
        let storage = Arc::clone(&storage);
        tokio::spawn(async move {
            tokio::select! {
                () = timer.cancelled() => {}
                () = tokio::time::sleep(MANUAL_AUTH_TIMEOUT) => {
                    if let Err(error) =
                        clear_pending_auth(&runtime, &server_name, Some(&oauth_state), &base_dir, Some(&storage)).await
                    {
                        tracing::error!("MCP Auth: Timed-out flow cleanup failed: {error}");
                    }
                }
            }
        });
    }
    Ok(())
}

/// `clearPendingAuth(runtime, serverName, oauthState?, fallbackStorageOptions)`
/// (`mcp-auth-flow.ts:472`).
///
/// **The state guard is the important half.** When `oauth_state` is given and the *recorded* state
/// differs, this returns without doing anything — that is what stops a stale five-minute timer from
/// destroying a **newer** flow for the same server. Dropping it is a use-after-free-class bug
/// (MCP-329).
///
/// **Named delta.** Upstream's last step clears the *persisted* `oauthState` when it still equals
/// the state being released. Under rmcp the PKCE verifier and CSRF token live in an
/// [`InMemoryStateStore`] that dies with the session (MCP-320: upstream keeps this material in
/// memory only, deliberately, and persisting it would be a regression), so there is no persisted
/// state to clear and the step dissolves.
async fn clear_pending_auth(
    runtime: &Arc<McpOAuthRuntime>,
    server_name: &str,
    oauth_state: Option<&str>,
    fallback_base_dir: &Path,
    fallback_storage: Option<&Arc<dyn McpOAuthStorage>>,
) -> McpResult<()> {
    let key = pending_auth_key(server_name, fallback_base_dir);

    let (session, state_to_release, storage) = {
        let mut state = runtime.state.lock().await;
        let recorded = state.pending_auth_states.get(&key).cloned();
        if let (Some(requested), Some(recorded)) = (oauth_state, recorded.as_deref())
            && requested != recorded
        {
            return Ok(());
        }

        if let Some(timer) = state.pending_auth_timers.remove(&key) {
            timer.cancel();
        }
        let removed = state.pending_auths.remove(&key);
        // `pendingAuth?.authStorageOptions ?? fallbackStorageOptions` (`mcp-auth-flow.ts:478`) —
        // **the pending flow's own storage wins.** A flow started against one auth directory must
        // be reclaimed from that directory even when the caller's fallback points somewhere else,
        // or a `--mcp-config` switch mid-flow strands the nonce.
        let storage = removed
            .as_ref()
            .map(|pending| Arc::clone(&pending.storage))
            .or_else(|| fallback_storage.map(Arc::clone));
        state.pending_auth_states.remove(&key);
        (
            removed.map(|pending| pending.session),
            recorded.or_else(|| oauth_state.map(str::to_string)),
            storage,
        )
    };

    // `pendingAuth.authProvider.deactivate()` — every later call on a deactivated flow refuses.
    if let Some(session) = session {
        let mut guard = session.lock().await;
        *guard = None;
    }
    if let Some(state_to_release) = state_to_release {
        cancel_pending_callback(&state_to_release);
        // `if (storedState === stateToRelease) await clearOAuthState(...)`
        // (`mcp-auth-flow.ts:494-497`). The equality test is what keeps this from deleting a
        // *newer* flow's persisted nonce when a stale timer finally fires.
        if let Some(storage) = storage
            && storage
                .oauth_state(server_name)
                .await?
                .is_some_and(|stored| stored == state_to_release)
        {
            storage.clear_oauth_state(server_name).await?;
        }
    }
    Ok(())
}

/// `generateState()` (`mcp-auth-flow.ts:142`) — 32 CSPRNG bytes rendered as **64 lowercase hex
/// characters**.
///
/// Nothing validates the format on the way back in (the callback matches by map lookup), so a port
/// that substituted base64url would pass every test while emitting URLs some strict authorization
/// servers reject on character class.
///
/// Reuses `cyrup_provider::auth::oauth::random`, whose `cfg`-free implementation is the workspace's
/// one entropy source (MCP-343 refuted the "no entropy on non-unix" finding against it).
pub fn generate_state() -> McpResult<String> {
    let bytes = cyrup_provider::auth::oauth::random::random_bytes(32)
        .map_err(|error| McpError::other(error.to_string()))?;
    let mut out = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    Ok(out)
}

// ===================================================================================================
// 9 · Driving rmcp: discovery, registration, the client-auth lever (MCP-309 … MCP-325)
// ===================================================================================================

/// The eight reserved authorization parameters (`mcp-oauth-provider.ts:121`).
///
/// The upstream documentation enumerates seven; the code's set has **eight**, adding
/// `code_challenge_method` (13g §14 item 8). An incomplete enumeration of a security-relevant
/// deny-list reads as exhaustive, so the doc is the thing that gets corrected, not this list.
pub const RESERVED_AUTHORIZATION_PARAMS: [&str; 8] = [
    "client_id",
    "code_challenge",
    "code_challenge_method",
    "redirect_uri",
    "resource",
    "response_type",
    "scope",
    "state",
];

/// `addAuthorizationParams(authorizationUrl, params)` (`mcp-oauth-provider.ts:130`) — MCP-316.
///
/// `oauth.authorizationParams` may add provider extensions but may never override a flow-owned
/// parameter. Because rmcp builds the URL and this decorates it afterwards, the **"already present
/// on the URL"** half of the check does all the work and must not be dropped.
///
/// Note `prompt` is deliberately *not* reserved: rmcp appends the `offline_access` scope (SEP-2207)
/// but emits no `prompt=consent` where upstream appends one for an `offline_access` scope, so a
/// Google-class provider that needs it gets it through this door (MCP-317).
pub fn add_authorization_params(
    authorization_url: &str,
    params: Option<&std::collections::BTreeMap<String, String>>,
) -> McpResult<String> {
    let Some(params) = params else {
        return Ok(authorization_url.to_string());
    };
    if params.is_empty() {
        return Ok(authorization_url.to_string());
    }
    let mut url = url::Url::parse(authorization_url)
        .map_err(|_| McpError::other(format!("Invalid authorization URL: {authorization_url}")))?;

    for (key, value) in params {
        let reserved = RESERVED_AUTHORIZATION_PARAMS.contains(&key.as_str());
        let present = url.query_pairs().any(|(existing, _)| existing == key.as_str());
        if reserved || present {
            return Err(McpError::other(format!(
                "OAuth authorizationParams.{key} cannot override an authorization flow parameter"
            )));
        }
        url.query_pairs_mut().append_pair(key, value);
    }
    Ok(url.to_string())
}

/// The `state` (CSRF) parameter rmcp put on the authorization URL.
///
/// This is the value the browser comes back with, and therefore the key the callback multiplexer
/// routes on — see the module header for why it is not [`generate_state`]'s nonce.
fn csrf_token_of(authorization_url: &str) -> Option<String> {
    let url = url::Url::parse(authorization_url).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
}

/// Resolve the configured client secret, running a `!command` **once** (MCP-349).
fn resolve_client_secret(server_name: &str, config: &OAuthFlowConfig) -> McpResult<Option<String>> {
    let Some(raw) = config.client_secret.as_deref() else {
        return Ok(None);
    };
    let context = format!("MCP server \"{server_name}\" OAuth clientSecret");
    resolve_command_secret(raw, &context).map(Some)
}

/// The metadata lever of MCP-318 / OA-14.
///
/// `AuthorizationManager::configure_client` selects request-body auth only when the authorization
/// server advertises `client_secret_post` **without** `client_secret_basic` — the TypeScript SDK's
/// rule. The adapter's rule differs in one place that matters: for an **empty or absent**
/// `token_endpoint_auth_methods_supported` the adapter picks `client_secret_post` where rmcp leaves
/// the oauth2 crate's HTTP Basic default. Against a server that publishes no supported-methods
/// list, the port would send Basic where upstream sends a body — a silent authentication failure
/// that looks like a bad secret.
///
/// There is no `addClientAuthentication` hook in rmcp; the lever that exists is
/// [`AuthorizationManager::set_metadata`], so the list is injected before `configure_client` runs.
fn apply_client_auth_lever(metadata: &mut AuthorizationMetadata, has_secret: bool) {
    if !has_secret {
        return;
    }
    let advertised = metadata
        .additional_fields
        .get("token_endpoint_auth_methods_supported")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|methods| !methods.is_empty());
    if advertised {
        return;
    }
    metadata.additional_fields.insert(
        "token_endpoint_auth_methods_supported".to_string(),
        serde_json::json!(["client_secret_post"]),
    );
}

/// `authorization_response_iss_parameter_supported === true` (RFC 9207).
fn requires_issuer(metadata: &AuthorizationMetadata) -> bool {
    metadata
        .additional_fields
        .get("authorization_response_iss_parameter_supported")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// `issuersMatch(first, second)` (`mcp-oauth-provider.ts:67`) — equality with exactly one trailing
/// slash tolerated on either side.
#[must_use]
pub fn issuers_match(first: &str, second: &str) -> bool {
    first == second
        || first.strip_suffix('/') == Some(second)
        || second.strip_suffix('/') == Some(first)
}

/// Re-apply the full client configuration after `initialize_from_store` — MCP-314.
///
/// `initialize_from_store` restores only `client_id`, through `configure_client_id`, which also
/// resets `redirect_uri` to the manager's base URL. A confidential or explicitly-redirected client
/// is therefore mis-configured after a restart unless the port re-applies it, which is what this
/// does from the [`OAuthClientRecord`] second record.
///
/// Upstream's expired-secret rule is kept: a stored `client_secret_expires_at` in the past means
/// re-register rather than reuse, so the record is skipped entirely.
fn restore_client_configuration(
    manager: &mut AuthorizationManager,
    record: &StoredClientInfo,
    redirect_uri: &str,
    configured_secret: Option<&str>,
) -> McpResult<()> {
    // Upstream's expired-secret rule: a stored `client_secret_expires_at` in the past means
    // re-register rather than reuse.
    if client_secret_expired(record, now_secs() as f64) {
        return Ok(());
    }
    // `to_oauth_client_config` carries the stub suppression: a secretless SEP-2352 issuer stub
    // written by the config-`clientId` path is only usable *paired with* the config that supplies
    // the secret, and re-applying it as standalone client information is the `invalid_client` bug
    // MCP-314 exists to avoid. When this call **does** hold the configured secret, that is exactly
    // the pairing, so the stub is rebuilt with it instead of skipped.
    let config = match record.to_oauth_client_config(redirect_uri) {
        Some(config) => match configured_secret {
            Some(secret) => OAuthClientConfig::new(record.client_id.clone(), redirect_uri)
                .with_client_secret(secret),
            None => config,
        },
        None => match configured_secret {
            Some(secret) => OAuthClientConfig::new(record.client_id.clone(), redirect_uri)
                .with_client_secret(secret),
            None => return Ok(()),
        },
    };
    manager
        .configure_client(config)
        .map_err(|error| McpError::other(error.to_string()))
}

/// The message MCP-322 restores when rmcp silently clears instead of throwing.
fn issuer_changed_message(server_name: &str) -> String {
    format!(
        "OAuth authorization server issuer changed for {server_name}; clear credentials before authenticating again"
    )
}

/// How a browser is opened. Injected so MCP-327's cases are assertable and so a headless host can
/// substitute a no-op without losing the printed URL.
pub trait BrowserLauncher: Send + Sync + 'static {
    /// Open `url` in the user's browser. Blocking; called from a blocking task.
    fn open(&self, url: &str) -> McpResult<()>;
}

/// The production launcher — the `opener` crate, called **directly**.
///
/// MCP-338: a native crate is not sandboxed, so this is not an ambient-authority question and does
/// not go through `HostServices::exec`. Note this is deliberately *not* the adapter's `openUrl`
/// helper, which dispatches per-platform through `pi.exec` and honours a `browser` override and
/// `$BROWSER`; the OAuth site uses the simpler one.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenerLauncher;

impl BrowserLauncher for OpenerLauncher {
    fn open(&self, url: &str) -> McpResult<()> {
        opener::open(url).map_err(|error| McpError::other(error.to_string()))
    }
}

/// A launcher that does nothing successfully — for headless hosts and tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopLauncher;

impl BrowserLauncher for NoopLauncher {
    fn open(&self, _url: &str) -> McpResult<()> {
        Ok(())
    }
}

/// `options.onAuthorizationUrl` — surface the URL before the browser handoff.
pub type AuthorizationUrlHook =
    Arc<dyn Fn(String) -> BoxFuture<'static, McpResult<()>> + Send + Sync>;

/// `options.onAuthorizationInput` — the manual paste prompt.
///
/// The [`CancelToken`] is the prompt's own controller: it is cancelled however the callback race
/// ends, so the prompt is dismissed the moment the loopback callback wins.
pub type AuthorizationInputHook =
    Arc<dyn Fn(String, CancelToken) -> BoxFuture<'static, McpResult<Option<String>>> + Send + Sync>;

/// `AuthenticateOptions` (`mcp-auth-flow.ts:50`).
#[derive(Clone)]
pub struct AuthenticateOptions {
    /// The runtime that owns the flow. `None` adopts the module-level legacy runtime.
    pub runtime: Option<Arc<McpOAuthRuntime>>,
    /// The caller's own abort, combined with the runtime's.
    pub signal: Option<CancelToken>,
    /// Where credentials live.
    pub storage: Arc<dyn McpOAuthStorage>,
    /// Called with the authorization URL **before** the browser is opened. When absent the URL is
    /// logged, because a remote user must never be stranded.
    pub on_authorization_url: Option<AuthorizationUrlHook>,
    /// The manual paste prompt. When absent the flow simply awaits the loopback callback.
    pub on_authorization_input: Option<AuthorizationInputHook>,
    /// How to open the browser.
    pub launcher: Arc<dyn BrowserLauncher>,
    /// The `WWW-Authenticate` value from the connect attempt's 401, when there was one — MCP-309's
    /// **reactive** discovery path, which costs no extra round trip.
    pub challenge: Option<String>,
    /// Forwarded to `set_allow_missing_issuer`; see [`start_auth`] for the narrowing.
    pub skip_issuer_metadata_validation: bool,
}

impl std::fmt::Debug for AuthenticateOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthenticateOptions")
            .field("runtime", &self.runtime.is_some())
            .field("has_url_hook", &self.on_authorization_url.is_some())
            .field("has_input_hook", &self.on_authorization_input.is_some())
            .field("challenge", &self.challenge.is_some())
            .field(
                "skip_issuer_metadata_validation",
                &self.skip_issuer_metadata_validation,
            )
            .finish()
    }
}

impl AuthenticateOptions {
    /// The minimum: a store and the real browser launcher.
    #[must_use]
    pub fn new(storage: Arc<dyn McpOAuthStorage>) -> Self {
        Self {
            runtime: None,
            signal: None,
            storage,
            on_authorization_url: None,
            on_authorization_input: None,
            launcher: Arc::new(OpenerLauncher),
            challenge: None,
            skip_issuer_metadata_validation: false,
        }
    }

    /// The combined abort — `combineAbortSignals(runtime.signal, options.signal)`.
    fn combined_signal(&self, runtime: &Arc<McpOAuthRuntime>) -> CancelToken {
        crate::abort::combine(runtime.token(), self.signal.as_ref())
    }
}

/// A built rmcp session plus everything the flow captured while building it.
struct PreparedSession {
    session: AuthorizationSession,
    expected_issuer: Option<String>,
    requires_issuer: bool,
    /// The client id the session ended up using, for the registration record.
    client_id: Option<String>,
}

/// Build the `rmcp` authorization session — the port's whole "acquisition" leg (13g §7 steps 4-8).
///
/// The ordering is the specification: the callback listener is already bound (step 3), so
/// `redirect_uri` is the port the listener actually took; discovery is seeded from the connect
/// attempt's challenge when there was one (MCP-309, recommendation (a)); the client-auth lever is
/// applied to the metadata **before** `configure_client` sees it (MCP-318); the store is restored
/// and its full client configuration re-applied (MCP-314); and only then does
/// `AuthorizationSession::new` run the MCP client-registration priority order — pre-registered,
/// then CIMD, then dynamic registration.
///
/// **`skipIssuerMetadataValidation` maps only partially.** The adapter's flag skips the RFC 8414
/// §3.3 issuer-echo check outright; rmcp's `set_allow_missing_issuer(true)` tolerates only a
/// **missing** `issuer` field, and a mismatched one still fails. That is recommendation (a) from
/// 13g "What does not fit cleanly": the safer behaviour, affecting a smaller set of servers than
/// the ones that simply omit the field. Named in the ported documentation as a deliberate
/// narrowing.
///
/// **`client_uri` / `logo_uri` do not reach the registration body.** rmcp's
/// `ClientRegistrationRequest` is fixed (`client_name`, `redirect_uris`, `grant_types`,
/// `token_endpoint_auth_method: "none"`, `response_types`, `scope`, `application_type`) and the
/// only lever that would recover the two fields is performing the registration POST here — which
/// needs an HTTP client this crate does not currently depend on. Recorded as
/// `TODO(MCP-312/OA-13)`; the report carries the one-line `Cargo.toml` ask.
#[allow(clippy::too_many_arguments)]
async fn prepare_session(
    server_name: &str,
    server_url: &str,
    config: &OAuthFlowConfig,
    storage: &Arc<dyn McpOAuthStorage>,
    redirect_uri: &str,
    challenge: Option<&str>,
    skip_issuer_metadata_validation: bool,
) -> McpResult<Option<PreparedSession>> {
    let mut manager = AuthorizationManager::new(server_url)
        .await
        .map_err(|error| McpError::other(error.to_string()))?;

    manager.set_credential_store(ServerCredentialStore::new(
        Arc::clone(storage),
        server_name,
        server_url,
    ));
    // MCP-320: upstream keeps the PKCE verifier and CSRF token in memory only, deliberately.
    // Persisting them would be a regression.
    manager.set_state_store(InMemoryStateStore::new());
    if skip_issuer_metadata_validation || config.skip_issuer_metadata_validation == Some(true) {
        manager.set_allow_missing_issuer(true);
    }

    // MCP-309(a): the reactive path. `resolve_metadata_from_challenge(None)` falls through to the
    // proactive `.well-known` walk, which is what `/mcp-auth` on a disconnected server wants.
    //
    // TODO(MCP-309): upstream's `probeAuthDiscovery` also exists — a 5000 ms-budgeted JSON-RPC
    // `initialize` POST whose only purpose is to read `WWW-Authenticate`, with headers filtered so
    // `!command` expressions are neither executed nor sent (`!!literal` kept, minus one `!`).
    // Recommendation (a) removes its only two call sites: the connect path takes the challenge off
    // the failed connect, and the disconnected path uses rmcp's own discovery walk. If a probe is
    // ever reintroduced (the setup panel's endpoint probe is the candidate), the header-filtering
    // rule comes with it — [`interpolate_secret_expression`] is already the filter.
    let resolution = manager
        .resolve_metadata_from_challenge(challenge)
        .await
        .map_err(|error| McpError::other(error.to_string()))?;
    let mut metadata = resolution.metadata;
    let configured_secret = resolve_client_secret(server_name, config)?;
    apply_client_auth_lever(&mut metadata, configured_secret.is_some());
    let expected_issuer = metadata.issuer.clone();
    let requires_issuer = requires_issuer(&metadata);
    manager.set_metadata(metadata);

    // MCP-322: rmcp *clears* where upstream *throws*. A store that held tokens and comes back
    // `false` means the issuer moved underneath them; surface upstream's actionable message rather
    // than letting a silent re-login look like a random logout.
    let had_tokens = storage
        .get_auth_for_url(server_name, server_url)
        .await?
        .and_then(|entry| entry.credentials)
        .and_then(|credentials| credentials.token_response)
        .is_some();
    let restored = manager
        .initialize_from_store()
        .await
        .map_err(|error| McpError::other(error.to_string()))?;
    if had_tokens && !restored {
        return Err(McpError::other(issuer_changed_message(server_name)));
    }

    // MCP-314: put the redirect URI and the secret back.
    if let Some(record) = storage
        .get_auth_for_url(server_name, server_url)
        .await?
        .and_then(|entry| entry.client)
    {
        restore_client_configuration(
            &mut manager,
            &record,
            redirect_uri,
            configured_secret.as_deref(),
        )?;
    }

    // `runSdkAuth` answers `AUTHORIZED` without a browser when the stored token is still live, or
    // when a refresh succeeds — `startAuth` step 11. The short-circuit has to happen **before**
    // `AuthorizationSession::new`, which would otherwise dynamically register a *second* client
    // for a server that is already authorized.
    if restored {
        match manager.get_access_token().await {
            Ok(_) => return Ok(None),
            // A store failure is fail-closed: it must not read as "not authorized yet".
            Err(error @ AuthError::InternalError(_)) => {
                return Err(McpError::other(error.to_string()));
            }
            // Everything else — no refresh token, a rejected one, a transient failure — falls
            // through to a fresh authorization, exactly as the SDK's `auth()` does.
            Err(error) => {
                tracing::debug!("MCP Auth: stored credentials unusable for {server_name}: {error}");
            }
        }
    }

    let mut request = AuthorizationRequest::new(redirect_uri.to_string())
        .with_client_name(config.client_name.clone().unwrap_or_else(app_name));
    if let Some(scope) = config.scope.as_deref() {
        // Passing scopes explicitly still appends `offline_access` when the AS advertises it
        // (SEP-2207); passing none lets rmcp auto-select from the challenge hint, the
        // protected-resource metadata or the AS metadata.
        request = request.with_scopes(scope.split_whitespace().map(str::to_string));
    }
    if let Some(client_id) = config.client_id.as_deref() {
        request = request.with_preregistered_client(client_id.to_string());
        if let Some(secret) = configured_secret.as_deref() {
            request = request.with_client_secret(secret.to_string());
        }
    }
    // The adapter publishes no CIMD document, so no `client_metadata_url` is supplied and rmcp
    // falls through to dynamic client registration.
    //
    // TODO(MCP-312): rmcp's `ClientRegistrationRequest` is fixed, so `client_uri`, `logo_uri` and
    // a confidential `token_endpoint_auth_method` never reach the registration body — the consent
    // screen shows the app name and no homepage or logo. Recovering them is 13g "What does not fit
    // cleanly" option (b): perform the registration POST here with the full body and hand the
    // result to `AuthorizationManager::configure_client`. That needs an HTTP client this crate
    // does not depend on yet (`reqwest = { workspace = true }`, one line in `Cargo.toml`), which
    // is why it is filed rather than done.

    let session = AuthorizationSession::new(manager, request)
        .await
        .map_err(|(_manager, error)| McpError::other(error.to_string()))?;

    let client_id = session
        .get_credentials()
        .await
        .ok()
        .map(|(client_id, _)| client_id)
        .or_else(|| config.client_id.clone());

    Ok(Some(PreparedSession {
        session,
        expected_issuer,
        requires_issuer,
        client_id,
    }))
}

/// The `client_credentials` short-circuit (13g §10, MCP-325).
///
/// No callback listener, no `state`, no browser, and the RFC 9207 check is skipped entirely.
/// Machine-to-machine servers authenticate with no interaction, including in `rpc`/`print` modes
/// where auto-auth explicitly allows it.
///
/// The stale-registration hygiene that runs first is the same rule as the interactive path but
/// **without** the redirect-URI check.
async fn authenticate_client_credentials(
    server_name: &str,
    server_url: &str,
    config: &OAuthFlowConfig,
    storage: &Arc<dyn McpOAuthStorage>,
) -> McpResult<()> {
    let stored = storage.get_auth_for_url(server_name, server_url).await?;
    if let Some(entry) = &stored
        && entry.client.is_some()
        && entry
            .credentials
            .as_ref()
            .and_then(|credentials| credentials.token_response.as_ref())
            .is_none()
        && config.client_id.is_none()
    {
        storage.save_client(server_name, server_url, None).await?;
    }

    let client_id = config.client_id.clone().ok_or_else(|| {
        McpError::other(format!(
            "MCP server \"{server_name}\" client_credentials OAuth requires oauth.clientId"
        ))
    })?;
    let client_secret = resolve_client_secret(server_name, config)?.ok_or_else(|| {
        McpError::other(format!(
            "MCP server \"{server_name}\" client_credentials OAuth requires oauth.clientSecret"
        ))
    })?;

    let mut state = OAuthState::new(server_url, None)
        .await
        .map_err(|error| McpError::other(error.to_string()))?;
    if let OAuthState::Unauthorized(manager) = &mut state {
        manager.set_credential_store(ServerCredentialStore::new(
            Arc::clone(storage),
            server_name,
            server_url,
        ));
        if config.skip_issuer_metadata_validation == Some(true) {
            manager.set_allow_missing_issuer(true);
        }
    }

    state
        .authenticate_client_credentials(ClientCredentialsConfig::ClientSecret {
            client_id,
            client_secret,
            scopes: config
                .scope
                .as_deref()
                .map(|scope| scope.split_whitespace().map(str::to_string).collect())
                .unwrap_or_default(),
            resource: None,
        })
        .await
        .map_err(|error| match error {
            // `if (result !== "AUTHORIZED") throw new UnauthorizedError("Failed to authorize")`.
            AuthError::AuthorizationRequired => McpError::other("Failed to authorize"),
            other => McpError::other(other.to_string()),
        })
}

// ===================================================================================================
// 10 · The flow: startAuth, authenticate, completeAuth (MCP-326 … MCP-332)
// ===================================================================================================

/// The persisted-`oauthState` clear that upstream performs at three cleanup sites.
///
/// Under rmcp the CSRF token and PKCE verifier live in an [`InMemoryStateStore`] that dies with the
/// session (MCP-320), so there is nothing on disk to clear and this is a no-op. It is kept as a
/// call — rather than deleted — because the three aggregate sites exist precisely to preserve *its*
/// failure alongside the primary error, and section 13f may yet give it a body.
async fn clear_persisted_oauth_state(
    storage: &Arc<dyn McpOAuthStorage>,
    server_name: &str,
) -> McpResult<()> {
    storage.clear_oauth_state(server_name).await
}

/// `startAuth(serverName, serverUrl, definition, options)` (`mcp-auth-flow.ts:320`) — MCP-328.
///
/// Returns the authorization URL, or an **empty string** when a live token or a successful refresh
/// meant no browser round trip was needed.
///
/// The fourteen steps of 13g §9 in order, with the two that are most defect-prone called out:
///
/// * **step 7 must precede step 8** — the listener binds before the redirect URI is fixed, so the
///   URI advertised to the authorization server is the port the listener actually took;
/// * **step 9's tokens-present branch deliberately discards the tokens** when the stored
///   `redirectUris` do not include the current redirect URL, because those tokens belong to a
///   client whose registered redirect URI no longer matches. Inverting or mis-computing that
///   condition destroys a working credential on an ordinary login.
pub async fn start_auth(
    server_name: &str,
    server_url: &str,
    definition: Option<&ServerEntry>,
    options: &AuthenticateOptions,
) -> McpResult<String> {
    if definition.is_some_and(ServerEntry::is_disabled) {
        return Err(McpError::other(format!(
            "MCP server \"{server_name}\" is disabled"
        )));
    }
    let runtime = get_runtime(options.runtime.as_ref())?;
    let config = definition
        .map(extract_oauth_config)
        .transpose()?
        .unwrap_or_default();
    let storage = Arc::clone(&options.storage);
    let base_dir = storage.base_dir();
    let signal = options.combined_signal(&runtime);
    let generation = runtime.generation();
    throw_if_aborted(&signal, None)?;

    // Step 3 — the `client_credentials` short-circuit: no listener, no state, no browser.
    if config.uses_client_credentials() {
        authenticate_client_credentials(server_name, server_url, &config, &storage).await?;
        return Ok(String::new());
    }

    // Step 4 — idempotency. An existing pending auth for the key whose `serverUrl` matches returns
    // its stored URL unchanged; a *different* URL falls through and is replaced.
    let key = pending_auth_key(server_name, &base_dir);
    {
        let state = runtime.state.lock().await;
        if let Some(existing) = state.pending_auths.get(&key)
            && existing.server_url == server_url
        {
            return Ok(existing.authorization_url.clone());
        }
    }

    // Step 5 — an explicit `redirectUri` drives both the bind and the registered metadata.
    let redirect_endpoint = config
        .redirect_uri
        .as_deref()
        .map(parse_oauth_redirect_uri)
        .transpose()?;
    // Step 6 — the adapter's own nonce. See the module header: rmcp mints the CSRF token that ends
    // up on the wire, so this one only carries the reservation across the bind.
    let bind_state = generate_state()?;

    // Step 7 — bind, reserving the state inside the same critical section.
    let ensure = EnsureCallbackServer {
        strict_port: config.strict_port(),
        port: redirect_endpoint.as_ref().map(|endpoint| endpoint.port),
        callback_host: redirect_endpoint
            .as_ref()
            .map(|endpoint| endpoint.callback_host.clone()),
        callback_path: redirect_endpoint
            .as_ref()
            .map(|endpoint| endpoint.callback_path.clone()),
        oauth_state: Some(bind_state.clone()),
        reserve_state: true,
    };
    if let Err(error) = ensure_callback_server(ensure)
        .await
        .and_then(|()| throw_if_aborted(&signal, None))
    {
        release_callback_server(&bind_state);
        if let Err(cleanup) = clear_persisted_oauth_state(&storage, server_name).await {
            return Err(aggregate_error(PHASE_STARTUP_CLEANUP, error, cleanup));
        }
        return Err(error);
    }

    // Step 8 — the redirect URI, read **after** the bind.
    let redirect_uri = match config.redirect_uri.clone() {
        Some(configured) => configured,
        None => match callback_redirect_uri().await {
            Some(uri) => uri,
            None => {
                release_callback_server(&bind_state);
                return Err(McpError::other(
                    "OAuth callback server did not report an assigned port",
                ));
            }
        },
    };

    let result = start_auth_inner(
        &runtime,
        server_name,
        server_url,
        &config,
        &storage,
        options,
        &redirect_uri,
        &bind_state,
        generation,
        &signal,
    )
    .await;

    match result {
        Ok(url) => Ok(url),
        Err(error) => {
            // Step 14 — the catch-all. Deactivate, then clear the pending auth; a failing cleanup
            // becomes an aggregate rather than hiding the primary error.
            release_callback_server(&bind_state);
            if let Err(cleanup) =
                clear_pending_auth(&runtime, server_name, Some(&bind_state), &base_dir, Some(&storage)).await
            {
                return Err(aggregate_error(PHASE_STARTUP_CLEANUP, error, cleanup));
            }
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn start_auth_inner(
    runtime: &Arc<McpOAuthRuntime>,
    server_name: &str,
    server_url: &str,
    config: &OAuthFlowConfig,
    storage: &Arc<dyn McpOAuthStorage>,
    options: &AuthenticateOptions,
    redirect_uri: &str,
    bind_state: &str,
    generation: u64,
    signal: &CancelToken,
) -> McpResult<String> {
    // Step 9 — the stale-registration checks, only when stored client info exists **and no
    // `config.clientId`**.
    let stored = storage.get_auth_for_url(server_name, server_url).await?;
    if let Some(entry) = &stored
        && let Some(client) = &entry.client
        && config.client_id.is_none()
    {
        let has_tokens = entry
            .credentials
            .as_ref()
            .and_then(|credentials| credentials.token_response.as_ref())
            .is_some();
        if !has_tokens {
            // No tokens: the registration is worthless, re-register.
            storage.save_client(server_name, server_url, None).await?;
            clear_persisted_oauth_state(storage, server_name).await?;
        } else {
            let registered_here = client
                .redirect_uris
                .as_ref()
                .is_some_and(|uris| uris.iter().any(|uri| uri == redirect_uri));
            if !registered_here {
                // **The destructive branch.** A callback-endpoint change forces re-registration
                // *and* discards the tokens, because those tokens belong to a client whose
                // registered redirect URI no longer matches. Changing the callback port or adding
                // an `oauth.redirectUri` triggers exactly this.
                storage.save_client(server_name, server_url, None).await?;
                storage
                    .save_credentials(server_name, server_url, None)
                    .await?;
                clear_persisted_oauth_state(storage, server_name).await?;
            }
        }
    }
    throw_if_aborted(signal, None)?;

    // Step 10 — discovery and acquisition, under the caller's abort.
    let prepared = abortable(
        prepare_session(
            server_name,
            server_url,
            config,
            storage,
            redirect_uri,
            options.challenge.as_deref(),
            options.skip_issuer_metadata_validation,
        ),
        signal,
    )
    .await??;
    throw_if_aborted(signal, None)?;

    // Step 11 — a live refresh sufficed.
    let Some(prepared) = prepared else {
        release_callback_server(bind_state);
        clear_persisted_oauth_state(storage, server_name).await?;
        return Ok(String::new());
    };

    // Step 12 — the URL rmcp built, decorated with the configured extras (MCP-316).
    let authorization_url =
        add_authorization_params(prepared.session.get_authorization_url(), config.authorization_params.as_ref())?;

    // Persist the registration record rmcp's `StoredCredentials` drops (MCP-290/MCP-314). The
    // `redirect_uris` entry is what step 9's stale check reads on the next login.
    if let Some(client_id) = prepared.client_id.clone() {
        storage
            .save_client(
                server_name,
                server_url,
                Some(StoredClientInfo {
                    client_id,
                    client_secret: None,
                    client_id_issued_at: None,
                    client_secret_expires_at: None,
                    redirect_uris: Some(vec![redirect_uri.to_string()]),
                    issuer: prepared.expected_issuer.clone(),
                    // Absent, not `false`, so the field stays off the wire when unset — upstream's
                    // `optionalBoolean` + `if (configPreRegistered !== undefined)`.
                    config_pre_registered: config.client_id.is_some().then_some(true),
                }),
            )
            .await?;
    }

    // The reservation moves from this module's nonce onto rmcp's CSRF token, which is what the
    // browser will actually send back. Nothing can arrive on either state in between: no browser
    // has been opened yet.
    let oauth_state = csrf_token_of(&authorization_url).ok_or_else(|| {
        McpError::other("OAuth authorization URL was not provided")
    })?;
    release_callback_server(bind_state);
    reserve_callback_server(&oauth_state);

    // Step 13 — publish and return.
    let pending = PendingAuth {
        server_name: server_name.to_string(),
        server_url: server_url.to_string(),
        authorization_url: authorization_url.clone(),
        session: Arc::new(AsyncMutex::new(Some(prepared.session))),
        expected_issuer: prepared.expected_issuer,
        requires_issuer: prepared.requires_issuer,
        storage: Arc::clone(storage),
        base_dir: storage.base_dir(),
    };
    if let Err(error) = set_pending_auth(runtime, pending, &oauth_state, generation).await {
        release_callback_server(&oauth_state);
        return Err(error);
    }
    Ok(authorization_url)
}

/// Where an authorization response came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationSource {
    /// The loopback listener answered.
    Callback,
    /// The user pasted it.
    Manual,
}

/// The winner of the callback-versus-paste race.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationResponse {
    /// The code (and `iss`, when present).
    pub input: AuthorizationCodeInput,
    /// Which leg won.
    pub source: AuthorizationSource,
}

/// `getSearchParamsFromInput(input)` (`mcp-auth-flow.ts:501`).
///
/// Try to parse the whole string as a URL and take its query, then merge in any **fragment**
/// parameters not already present; on parse failure, treat the text after the first `?` (or the
/// whole string minus a leading `#`) as a query string and return it **only if** it contains
/// `code`, `state` or `error`.
///
/// Reuses `cyrup_provider::auth::oauth::query::parse_query` for the raw-query-string arm.
#[must_use]
pub fn get_search_params_from_input(input: &str) -> Option<Vec<(String, String)>> {
    if let Ok(url) = url::Url::parse(input) {
        let mut params: Vec<(String, String)> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        if let Some(fragment) = url.fragment() {
            let fragment = fragment.strip_prefix('#').unwrap_or(fragment);
            for (key, value) in cyrup_provider::auth::oauth::query::parse_query(fragment) {
                if !params.iter().any(|(existing, _)| existing == &key) {
                    params.push((key, value));
                }
            }
        }
        return Some(params);
    }

    let query = match input.find('?') {
        Some(index) => input.get(index + 1..).unwrap_or_default(),
        None => input.strip_prefix('#').unwrap_or(input),
    };
    let params = cyrup_provider::auth::oauth::query::parse_query(query);
    let interesting = params
        .iter()
        .any(|(key, _)| key == "code" || key == "state" || key == "error");
    interesting.then_some(params)
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// `parseAuthorizationRedirectInput(input, expectedState?)` (`mcp-auth-flow.ts:526`) — MCP-326.
///
/// Accepts a raw code, a query string, or the full localhost redirect URL copied from the browser
/// address bar. rmcp's `AuthorizationCallback::from_redirect_url` covers only the strict "a real
/// URL with `code` and `state`" case, with its own messages, and has no bare-code, fragment-merge
/// or expected-state comparison — it is **not** a substitute for this parser.
pub fn parse_authorization_redirect_input(
    input: &str,
    expected_state: Option<&str>,
) -> McpResult<AuthorizationCodeInput> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(McpError::other(
            "Authorization code or redirect URL is required",
        ));
    }

    if let Some(params) = get_search_params_from_input(trimmed) {
        if let Some(error) = param(&params, "error") {
            let message = match param(&params, "error_description") {
                Some(description) => format!("{error}: {description}"),
                None => error.to_string(),
            };
            return Err(McpError::other(message));
        }

        let state = param(&params, "state");
        if expected_state.is_some() && state.is_none() {
            return Err(McpError::other("OAuth state missing from redirect URL"));
        }
        if let Some(expected) = expected_state
            && state != Some(expected)
        {
            return Err(McpError::other(
                "OAuth state mismatch - potential CSRF attack",
            ));
        }

        if let Some(code) = param(&params, "code") {
            return Ok(AuthorizationCodeInput {
                code: code.to_string(),
                iss: param(&params, "iss").map(str::to_string),
            });
        }
    }

    // `/^[A-Za-z0-9._~+/=-]+$/` — a bare authorization code.
    let bare_code = !trimmed.is_empty()
        && trimmed.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '+' | '/' | '=' | '-')
        });
    if bare_code {
        return Ok(AuthorizationCodeInput {
            code: trimmed.to_string(),
            iss: None,
        });
    }

    Err(McpError::other(
        "Could not find an OAuth authorization code in the provided input",
    ))
}

/// Cancel a token however the enclosing scope exits — upstream's
/// `finally { inputController.abort() }`.
struct CancelOnDrop(CancelToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// `waitForAuthorizationResponse(...)` (`mcp-auth-flow.ts:583`) — MCP-326.
///
/// With no input prompt it simply awaits the callback. Otherwise it races the callback against the
/// prompt, wrapped in the caller's abort, with the prompt's own token cancelled in a scope guard
/// **so the prompt is dismissed however the race ends**.
///
/// A `Manual` winner with empty or whitespace input is `OAuth authentication cancelled`; one whose
/// input yields no parameters is
/// `Paste the full OAuth callback URL, including its code and state parameters` — **a bare code is
/// rejected on this path**, even though [`parse_authorization_redirect_input`] would accept one.
pub async fn wait_for_authorization_response(
    callback: oneshot::Receiver<McpResult<AuthorizationCodeInput>>,
    authorization_url: &str,
    expected_state: &str,
    on_authorization_input: Option<&AuthorizationInputHook>,
    signal: &CancelToken,
) -> McpResult<AuthorizationResponse> {
    let Some(on_authorization_input) = on_authorization_input else {
        let input = abortable(callback, signal).await?;
        let input = input.map_err(|_| McpError::other(CALLBACK_STOPPED))??;
        return Ok(AuthorizationResponse {
            input,
            source: AuthorizationSource::Callback,
        });
    };

    let prompt_token = CancelToken::new();
    let _guard = CancelOnDrop(prompt_token.clone());
    let prompt = on_authorization_input(authorization_url.to_string(), prompt_token);

    let winner = tokio::select! {
        biased;
        () = signal.cancelled() => {
            return Err(McpError::Aborted(crate::abort::ABORTED_FALLBACK_REASON.to_string()));
        }
        settled = callback => {
            let input = settled.map_err(|_| McpError::other(CALLBACK_STOPPED))??;
            return Ok(AuthorizationResponse { input, source: AuthorizationSource::Callback });
        }
        typed = prompt => typed?,
    };

    let typed = winner.unwrap_or_default();
    if typed.trim().is_empty() {
        return Err(McpError::other("OAuth authentication cancelled"));
    }
    if get_search_params_from_input(typed.trim()).is_none() {
        return Err(McpError::other(
            "Paste the full OAuth callback URL, including its code and state parameters",
        ));
    }
    Ok(AuthorizationResponse {
        input: parse_authorization_redirect_input(&typed, Some(expected_state))?,
        source: AuthorizationSource::Manual,
    })
}

/// `completeAuthFromInput(serverName, input, options)` (`mcp-auth-flow.ts:621`).
///
/// Resolves the runtime, looks up the pending state for the server, parses the input **against
/// that state**, and delegates to [`complete_auth`]. This is the `mcp({action:"auth-complete"})`
/// entry point.
pub async fn complete_auth_from_input(
    server_name: &str,
    input: &str,
    options: &AuthenticateOptions,
) -> McpResult<AuthStatus> {
    let runtime = get_runtime(options.runtime.as_ref())?;
    let signal = options.combined_signal(&runtime);
    throw_if_aborted(&signal, None)?;
    let key = pending_auth_key(server_name, &options.storage.base_dir());
    let oauth_state = {
        let state = runtime.state.lock().await;
        state.pending_auth_states.get(&key).cloned()
    };
    throw_if_aborted(&signal, None)?;
    let parsed = parse_authorization_redirect_input(input, oauth_state.as_deref())?;
    complete_auth(server_name, parsed, options).await
}

/// `completeAuth(serverName, code | {code, iss?}, options)` (`mcp-auth-flow.ts:640`) — MCP-331.
///
/// # The RFC 9207 gate, and why it is not merely message quality (MCP-323)
///
/// Steps 3 and 4 of 13g §8 run **before** the exchange, and the `iss`-required case is the **only**
/// path that leaves the pending auth alive so the user can paste again without restarting the flow.
/// It is modelled as an explicit flag, not an error-type match, because it is the only path that
/// skips the `finally` cleanup.
///
/// **Correction to the plan.** 13g asserts that rmcp validates the issuer *before* consuming the
/// stored state, which would make this gate purely cosmetic. In rmcp 3.1.2
/// (`transport/auth.rs`, `exchange_code_for_token_with_issuer`) the state is **deleted first** and
/// `validate_authorization_response_issuer` runs after it, so a callback bearing the correct
/// `state` but a missing required `iss` *would* discard the PKCE verifier the legitimate callback
/// still needs. This pre-check is therefore load-bearing: it refuses before rmcp can consume the
/// state, which is exactly what upstream's `keepPendingForRetry` exists to provide.
pub async fn complete_auth(
    server_name: &str,
    authorization_code: AuthorizationCodeInput,
    options: &AuthenticateOptions,
) -> McpResult<AuthStatus> {
    let runtime = get_runtime(options.runtime.as_ref())?;
    let signal = options.combined_signal(&runtime);
    throw_if_aborted(&signal, None)?;

    let fallback_base_dir = options.storage.base_dir();
    let key = pending_auth_key(server_name, &fallback_base_dir);

    // The **stored** options win over the caller's fallback for every later store operation.
    let (session, expected_issuer, requires_issuer, base_dir) = {
        let state = runtime.state.lock().await;
        let Some(pending) = state.pending_auths.get(&key) else {
            return Err(McpError::other(format!(
                "No pending OAuth flow for server: {server_name}"
            )));
        };
        (
            Arc::clone(&pending.session),
            pending.expected_issuer.clone(),
            pending.requires_issuer,
            pending.base_dir.clone(),
        )
    };
    let oauth_state = {
        let state = runtime.state.lock().await;
        state.pending_auth_states.get(&key).cloned()
    };
    throw_if_aborted(&signal, None)?;

    let mut keep_pending_for_retry = false;
    let outcome = async {
        let AuthorizationCodeInput { code, iss } = &authorization_code;

        // Step 3 — the AS requires `iss` and the user pasted a bare code.
        if expected_issuer.is_some() && iss.is_none() && requires_issuer {
            keep_pending_for_retry = true;
            return Err(McpError::other(format!(
                "The authorization server for {server_name} requires the RFC 9207 \"iss\" parameter. Paste the full redirect URL from the browser address bar (not just the authorization code)."
            )));
        }
        // Step 4 — a mismatched `iss`. The flow is **not** kept pending.
        if let (Some(expected), Some(received)) = (expected_issuer.as_deref(), iss.as_deref())
            && !issuers_match(expected, received)
        {
            return Err(McpError::other(format!(
                "The OAuth authorization response issuer does not match the discovered issuer for {server_name}."
            )));
        }

        let csrf_token = oauth_state.clone().ok_or_else(|| {
            McpError::other("OAuth state not found - this should not happen")
        })?;

        let guard = session.lock().await;
        let Some(session) = guard.as_ref() else {
            return Err(McpError::other("OAuth flow is no longer active"));
        };
        let exchange = session.handle_callback_with_issuer(code, &csrf_token, iss.as_deref());
        abortable(exchange, &signal)
            .await?
            .map_err(|error| match error {
                AuthError::AuthorizationRequired => McpError::other("Failed to authorize"),
                other => McpError::other(other.to_string()),
            })?;
        throw_if_aborted(&signal, None)?;
        Ok(AuthStatus::Authenticated)
    }
    .await;

    // The `finally`, which runs **unless** the retry flag is set.
    if keep_pending_for_retry {
        return outcome;
    }
    let cleanup =
        clear_pending_auth(&runtime, server_name, oauth_state.as_deref(), &base_dir, Some(&options.storage))
            .await;
    match (outcome, cleanup) {
        (Ok(status), Ok(())) => Ok(status),
        // The body succeeded and only the cleanup failed: rethrow the cleanup error alone.
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(aggregate_error(PHASE_COMPLETION_CLEANUP, error, cleanup))
        }
    }
}

/// `authenticate(serverName, serverUrl, definition, options)` (`mcp-auth-flow.ts:723`) — MCP-330.
///
/// The full round trip: [`start_auth`], the callback waiter, the browser, the race, and
/// [`complete_auth`]. Two tool calls that both hit `needs-auth` on the same server produce **one**
/// browser window, because an in-flight operation for
/// `` `${serverName}|${serverUrl}|${authBaseDir}` `` is returned as-is.
///
/// The order inside is load-bearing: **register the callback waiter, then surface the URL, then
/// open the browser** — always, so a remote user whose browser cannot reach the loopback listener
/// is never stranded, and so a callback that arrives while the URL is still being printed is not
/// dropped.
pub async fn authenticate(
    server_name: &str,
    server_url: &str,
    definition: Option<&ServerEntry>,
    options: &AuthenticateOptions,
) -> McpResult<AuthStatus> {
    if definition.is_some_and(ServerEntry::is_disabled) {
        return Err(McpError::other(format!(
            "MCP server \"{server_name}\" is disabled"
        )));
    }
    let runtime = get_runtime(options.runtime.as_ref())?;
    let signal = options.combined_signal(&runtime);
    throw_if_aborted(&signal, None)?;

    let base_dir = options.storage.base_dir();
    let auth_key = format!("{server_name}|{server_url}|{}", base_dir.display());

    // The in-flight dedup. `Shared` needs a `Clone` output, hence the `Arc<McpError>`.
    //
    // The lookup and the insert happen **under one lock**: building the future is synchronous (the
    // body does not run until it is polled), so there is no await between them and two concurrent
    // calls for the same key cannot both decide they are the first. Splitting them would produce
    // two browser windows for the one server MCP-330 exists to prevent.
    let (operation, joined) = {
        let mut state = runtime.state.lock().await;
        match state.pending_authentications.get(&auth_key) {
            Some(existing) => (existing.clone(), true),
            None => {
                let operation = {
                    let runtime = Arc::clone(&runtime);
                    let options = options.clone();
                    let definition = definition.cloned();
                    let server_name = server_name.to_string();
                    let server_url = server_url.to_string();
                    let signal = signal.clone();
                    async move {
                        authenticate_inner(
                            &runtime,
                            &server_name,
                            &server_url,
                            definition.as_ref(),
                            &options,
                            &signal,
                        )
                        .await
                        .map_err(Arc::new)
                    }
                    .boxed()
                    .shared()
                };
                state
                    .pending_authentications
                    .insert(auth_key.clone(), operation.clone());
                (operation, false)
            }
        }
    };

    if joined {
        return operation.await.map_err(|error| clone_error(&error));
    }

    let result = operation.clone().await;

    // Remove the dedup entry **only if it is still the one this call installed** — a later call
    // that replaced it must keep its own entry. `Shared::ptr_eq` is the identity test.
    {
        let mut state = runtime.state.lock().await;
        if let Some(current) = state.pending_authentications.get(&auth_key)
            && Shared::ptr_eq(current, &operation)
        {
            state.pending_authentications.remove(&auth_key);
        }
    }

    result.map_err(|error| clone_error(&error))
}

async fn authenticate_inner(
    runtime: &Arc<McpOAuthRuntime>,
    server_name: &str,
    server_url: &str,
    definition: Option<&ServerEntry>,
    options: &AuthenticateOptions,
    signal: &CancelToken,
) -> McpResult<AuthStatus> {
    let mut flow_options = options.clone();
    flow_options.runtime = Some(Arc::clone(runtime));
    flow_options.signal = Some(signal.clone());

    let authorization_url =
        start_auth(server_name, server_url, definition, &flow_options).await?;
    if authorization_url.is_empty() {
        // An empty URL means a live refresh already sufficed.
        return Ok(AuthStatus::Authenticated);
    }

    let base_dir = options.storage.base_dir();
    let key = pending_auth_key(server_name, &base_dir);
    let mut oauth_state: Option<String> = None;

    let result = async {
        // Read the state back **inside** the cleanup boundary: `start_auth` has already reserved
        // callback state at this point.
        oauth_state = {
            let state = runtime.state.lock().await;
            state.pending_auth_states.get(&key).cloned()
        };
        throw_if_aborted(signal, None)?;
        let Some(state_value) = oauth_state.clone() else {
            return Err(McpError::other("OAuth state not found - this should not happen"));
        };

        // Register the callback BEFORE opening the browser.
        let callback = wait_for_callback(&state_value);

        // Always surface the URL first so remote/headless users can copy it even when the OS
        // browser handoff is unavailable or invisible.
        match options.on_authorization_url.as_ref() {
            Some(hook) => abortable(hook(authorization_url.clone()), signal).await??,
            None => tracing::info!(
                "MCP Auth: Open this URL to authenticate {server_name}:\n{authorization_url}"
            ),
        }

        let launcher = Arc::clone(&options.launcher);
        let launch_url = authorization_url.clone();
        let launch = tokio::task::spawn_blocking(move || launcher.open(&launch_url));
        match abortable(launch, signal).await {
            Err(abort) => return Err(abort),
            Ok(Ok(Ok(()))) => {}
            Ok(outcome) => {
                // `if (isAbortError(error, signal)) throw error` — an abort propagates; every
                // other launch failure is logged and **the flow continues**, because the URL has
                // already been surfaced and the loopback callback may still arrive.
                if signal.is_cancelled() {
                    return Err(McpError::Aborted(
                        crate::abort::ABORTED_FALLBACK_REASON.to_string(),
                    ));
                }
                let message = match outcome {
                    Ok(Err(error)) => error.to_string(),
                    Err(join) => join.to_string(),
                    Ok(Ok(())) => String::new(),
                };
                tracing::warn!(
                    "MCP Auth: Failed to open browser for {server_name}; waiting for manual callback: {message}"
                );
            }
        }

        let response = wait_for_authorization_response(
            callback,
            &authorization_url,
            &state_value,
            options.on_authorization_input.as_ref(),
            signal,
        )
        .await?;
        if response.source == AuthorizationSource::Manual {
            cancel_pending_callback(&state_value);
        }

        // The callback listener accepted only the flow-local reserved state; manual input is
        // checked against the same state before token exchange.
        throw_if_aborted(signal, None)?;
        complete_auth(server_name, response.input, &flow_options).await
    }
    .await;

    match result {
        Ok(status) => Ok(status),
        Err(error) => {
            if let Some(state_value) = oauth_state.as_deref() {
                cancel_pending_callback(state_value);
            }
            if let Err(cleanup) =
                clear_pending_auth(runtime, server_name, oauth_state.as_deref(), &base_dir, Some(&options.storage)).await
            {
                return Err(aggregate_error(PHASE_CANCELLATION_CLEANUP, error, cleanup));
            }
            Err(error)
        }
    }
}

// ===================================================================================================
// 11 · Tokens, status, removal, and the public token API (MCP-324, MCP-332, MCP-346)
// ===================================================================================================

/// `getValidToken(serverName, serverUrl, options)` (`mcp-auth-flow.ts:828`) — MCP-324.
///
/// 1. no tokens ⇒ `None`;
/// 2. [`is_token_expired`] is **tri-state**: `Some(false)` returns the tokens untouched, with no
///    network call;
/// 3. expired **and** a refresh token exists ⇒ refresh, with the three log lines kept verbatim;
/// 4. the fall-through — no expiry information, or expired with no refresh token — **returns the
///    tokens anyway** ("assume valid"). This is deliberate, and it is the published contract
///    `oauth.ts`'s `getMcpOAuthTokensForUrl` is built on.
///
/// Abort errors and credential-store errors **rethrow**; everything else logs and answers `None`.
///
/// **Named delta.** Upstream's refresh context is built with an *empty* config and **no server
/// definition**, so no configured headers are sent on the refresh probe: for a server that gates
/// even its 401 behind a tenant header, login works and refresh silently fails. rmcp reproduces the
/// asymmetry for a different reason — its discovery uses its own HTTP client, not the transport's
/// configured headers.
pub async fn get_valid_token(
    server_name: &str,
    server_url: &str,
    options: &AuthenticateOptions,
) -> McpResult<Option<McpTokens>> {
    let runtime = get_runtime(options.runtime.as_ref())?;
    let signal = options.combined_signal(&runtime);
    throw_if_aborted(&signal, None)?;

    let storage = Arc::clone(&options.storage);
    let entry = storage.get_auth_for_url(server_name, server_url).await?;
    throw_if_aborted(&signal, None)?;
    let Some(tokens) = entry
        .as_ref()
        .and_then(|entry| entry.credentials.as_ref())
        .and_then(project_tokens)
    else {
        return Ok(None);
    };

    let expired = is_token_expired(Some(&tokens), now_secs());
    if expired == Some(false) {
        return Ok(Some(tokens));
    }

    if expired == Some(true) && tokens.refresh_token.is_some() {
        tracing::info!("MCP Auth: Token expired for {server_name}, attempting refresh");
        let client = entry.as_ref().and_then(|entry| entry.client.clone());
        if client.is_none() {
            tracing::info!("MCP Auth: No client info for refresh for {server_name}");
            return Ok(None);
        }

        match refresh_tokens(server_name, server_url, &storage, options, &signal).await {
            Ok(Some(refreshed)) => return Ok(Some(refreshed)),
            Ok(None) => return Ok(None),
            Err(error) => {
                // Abort errors and credential-store errors rethrow; everything else is logged.
                if is_abort_error(&error, Some(&signal)) {
                    return Err(error);
                }
                if error.to_string().starts_with(CREDENTIAL_STORE_PREFIX) {
                    return Err(error);
                }
                tracing::error!("MCP Auth: Token refresh failed for {server_name}: {error}");
                return Ok(None);
            }
        }
    }

    // No expiration info or no refresh token: assume valid.
    Ok(Some(tokens))
}

/// The marker `get_valid_token` classifies a credential-store failure by.
///
/// `ServerCredentialStore`'s errors reach rmcp as `AuthError::InternalError`, and MCP-324 requires
/// those to **rethrow** where a network or protocol failure is logged and answered `None`. Mapping
/// them through one prefix keeps that distinction structural rather than guessed.
const CREDENTIAL_STORE_PREFIX: &str = "credential store";

/// Map an rmcp auth error, tagging the store failures MCP-324 must not swallow.
fn map_auth_error(error: AuthError) -> McpError {
    match error {
        AuthError::InternalError(message) => {
            McpError::other(format!("{CREDENTIAL_STORE_PREFIX}: {message}"))
        }
        other => McpError::other(other.to_string()),
    }
}

async fn refresh_tokens(
    server_name: &str,
    server_url: &str,
    storage: &Arc<dyn McpOAuthStorage>,
    options: &AuthenticateOptions,
    signal: &CancelToken,
) -> McpResult<Option<McpTokens>> {
    let mut manager = AuthorizationManager::new(server_url)
        .await
        .map_err(map_auth_error)?;
    manager.set_credential_store(ServerCredentialStore::new(
        Arc::clone(storage),
        server_name,
        server_url,
    ));
    manager.set_state_store(InMemoryStateStore::new());
    if options.skip_issuer_metadata_validation {
        manager.set_allow_missing_issuer(true);
    }

    let resolution = abortable(manager.resolve_metadata(), signal)
        .await?
        .map_err(map_auth_error)?;
    manager.set_metadata(resolution.metadata);
    let restored = abortable(manager.initialize_from_store(), signal)
        .await?
        .map_err(map_auth_error)?;
    if !restored {
        // rmcp cleared the store on an issuer change (MCP-322), or there was nothing to restore.
        return Ok(None);
    }

    // The redirect URI does not participate in a refresh, but `configure_client_id` has already
    // overwritten it with the base URL, so the stored record is re-applied for its **secret**.
    if let Some(record) = storage
        .get_auth_for_url(server_name, server_url)
        .await?
        .and_then(|entry| entry.client)
    {
        let redirect = callback_redirect_uri()
            .await
            .unwrap_or_else(|| server_url.to_string());
        restore_client_configuration(&mut manager, &record, &redirect, None)?;
    }

    // rmcp keeps the existing refresh token when the response omits one (RFC 6749 §6) and writes
    // the result straight through the credential store.
    abortable(manager.refresh_token(), signal)
        .await?
        .map_err(map_auth_error)?;

    Ok(storage
        .get_auth_for_url(server_name, server_url)
        .await?
        .and_then(|entry| entry.credentials)
        .as_ref()
        .and_then(project_tokens))
}

/// `getAuthStatus(serverName, options)` (`mcp-auth-flow.ts:900`).
///
/// Resolves the runtime for its side effect of resurrecting an aborted legacy runtime, then answers
/// `NotAuthenticated` when no tokens are stored, else `expired ? Expired : Authenticated` — so a
/// `None` expiry reads as `Authenticated`.
pub async fn get_auth_status(
    server_name: &str,
    server_url: &str,
    options: &AuthenticateOptions,
) -> McpResult<AuthStatus> {
    let _runtime = get_runtime(options.runtime.as_ref())?;
    let tokens = options
        .storage
        .get_auth_for_url(server_name, server_url)
        .await?
        .and_then(|entry| entry.credentials)
        .as_ref()
        .and_then(project_tokens);
    let Some(tokens) = tokens else {
        return Ok(AuthStatus::NotAuthenticated);
    };
    Ok(match is_token_expired(Some(&tokens), now_secs()) {
        Some(true) => AuthStatus::Expired,
        _ => AuthStatus::Authenticated,
    })
}

/// `removeAuth(serverName, options)` (`mcp-auth-flow.ts:915`), in order, with abort checks
/// interleaved at four points.
pub async fn remove_auth(server_name: &str, options: &AuthenticateOptions) -> McpResult<()> {
    let runtime = get_runtime(options.runtime.as_ref())?;
    let signal = options.combined_signal(&runtime);
    throw_if_aborted(&signal, None)?;

    let base_dir = options.storage.base_dir();
    let key = pending_auth_key(server_name, &base_dir);
    let oauth_state = {
        let state = runtime.state.lock().await;
        state.pending_auth_states.get(&key).cloned()
    };
    throw_if_aborted(&signal, None)?;
    if let Some(state_value) = oauth_state.as_deref() {
        cancel_pending_callback(state_value);
    }
    clear_pending_auth(&runtime, server_name, oauth_state.as_deref(), &base_dir, Some(&options.storage))
        .await?;
    throw_if_aborted(&signal, None)?;
    options.storage.clear_all(server_name).await?;
    clear_persisted_oauth_state(&options.storage, server_name).await?;
    throw_if_aborted(&signal, None)?;
    tracing::info!("MCP Auth: Removed credentials for {server_name}");
    Ok(())
}

// **De-duplicated at integration (MCP-270).** This file had landed `oauth.ts:16`'s three-state
// report under the name `OAuthCredentialStatus`, which 13f's plan text assigns to the *entry*-
// carrying enum in `mcp-auth.ts`. The token-carrying one is `McpOAuthTokenStatus` and it lives in
// [`crate::credentials`] beside its sibling; this module re-exports both so the façade below reads
// the same as upstream's.
pub use crate::credentials::{McpOAuthTokenStatus, OAuthCredentialStatus};

/// `getMcpOAuthTokensForUrl(...)` (`oauth.ts`) — delegates to [`get_valid_token`], **so it may
/// refresh**. The store being unavailable throws rather than looking like "no tokens".
pub async fn get_mcp_oauth_tokens_for_url(
    server_name: &str,
    server_url: &str,
    options: &AuthenticateOptions,
) -> McpResult<Option<McpTokens>> {
    get_valid_token(server_name, server_url, options).await
}

/// `inspectMcpOAuthTokensForUrl(...)` (`oauth.ts`) — **never refreshes**, and exposes **only**
/// tokens: never the client registration, never the PKCE verifier, never the CSRF state.
pub async fn inspect_mcp_oauth_tokens_for_url(
    server_name: &str,
    server_url: &str,
    storage: &Arc<dyn McpOAuthStorage>,
) -> McpOAuthTokenStatus {
    match storage.get_auth_for_url(server_name, server_url).await {
        Ok(entry) => entry
            .and_then(|entry| entry.credentials)
            .as_ref()
            .and_then(project_tokens)
            .map_or(McpOAuthTokenStatus::Absent, McpOAuthTokenStatus::Present),
        // `inspectAuthForUrl`'s `unavailable` passes through unchanged — **never** as "no tokens",
        // which would restart an authorization the user already completed.
        Err(error) => McpOAuthTokenStatus::Unavailable {
            message: error.to_string(),
        },
    }
}

/// `updateMcpOAuthTokensForUrl(...)` (`oauth.ts`) — moving tokens to a new URL clears the old
/// entry's client registration, which the URL-binding purge rule in the store performs.
pub async fn update_mcp_oauth_tokens_for_url(
    server_name: &str,
    server_url: &str,
    credentials: StoredCredentials,
    storage: &Arc<dyn McpOAuthStorage>,
) -> McpResult<()> {
    storage
        .save_credentials(server_name, server_url, Some(credentials))
        .await
}

// ===================================================================================================
// 12 · The user-facing strings and the connect-path classification (MCP-333, MCP-334, MCP-335)
// ===================================================================================================

// TODO(MCP-334): `/mcp-auth <server>` registers at the command tier (`InitApi::register_command`
// plus `NativeExtension::execute_command`) and lives in `registration.rs`/`extension.rs`, not here.
// This module owns the literals and the guard order — no interactive UI, unknown server, disabled,
// not an OAuth server, no URL — and `HostServices::{set_status, notify, confirm, input}` do the
// rest. The `mcp-auth` status key is set to [`msg_authenticating`] and cleared in a `finally`
// **unless the signal aborted**. Terminal hyperlinks (OSC 8) have no helper: emit the escape
// sequence in the message text, or drop the hyperlink and record the loss.
//
// TODO(MCP-335): `mcp({action:"auth-start"/"auth-complete"})` and auto-auth live in `proxy.rs` and
// the direct-tools path. This module supplies [`format_manual_auth_instructions`],
// [`msg_auth_required_proxy`], [`msg_auth_required_direct_tools`] and [`msg_auto_auth_failed`];
// the `details` keys (`mode`, `error`, `server`, `authorizationUrl`, `authenticated`, `status`,
// `message`) are consumed by the tool-result renderer, so keep the names. `auth-complete`
// additionally **closes the server connection** so the next `connect` uses the new token.
//
// TODO(MCP-341): ship the corrected `OAUTH.md` with 13g §14's eight divergences fixed, plus this
// port's own deltas — the `skipIssuerMetadataValidation` narrowing, the absent `prompt=consent`,
// the `localhost`/`127.0.0.1` bind decision, and the `CYRUP_MCP_OAUTH_CALLBACK_PORT` rename.

/// `"OAuth authentication requires an interactive session."`
pub const MSG_REQUIRES_INTERACTIVE: &str = "OAuth authentication requires an interactive session.";

/// `Server "<n>" not found in config`
#[must_use]
pub fn msg_server_not_found(server_name: &str) -> String {
    format!("Server \"{server_name}\" not found in config")
}

/// `Server "<n>" is disabled. Run /mcp enable <n>, then /reload.`
#[must_use]
pub fn msg_server_disabled(server_name: &str) -> String {
    format!("Server \"{server_name}\" is disabled. Run /mcp enable {server_name}, then /reload.")
}

/// `Server "<n>" does not use OAuth authentication. Set "auth": "oauth" or omit auth for
/// auto-detection.` — the `notify` variant breaks after the first sentence with `\n`.
#[must_use]
pub fn msg_not_oauth(server_name: &str) -> String {
    format!(
        "Server \"{server_name}\" does not use OAuth authentication. Set \"auth\": \"oauth\" or omit auth for auto-detection."
    )
}

/// `Server "<n>" has no URL configured (OAuth requires HTTP transport)`
#[must_use]
pub fn msg_no_url(server_name: &str) -> String {
    format!("Server \"{server_name}\" has no URL configured (OAuth requires HTTP transport)")
}

/// The `mcp-auth` status-bar key's value while a login is running.
#[must_use]
pub fn msg_authenticating(server_name: &str) -> String {
    format!("Authenticating {server_name}...")
}

/// `OAuth authentication successful for "<n>".`
#[must_use]
pub fn msg_auth_success(server_name: &str) -> String {
    format!("OAuth authentication successful for \"{server_name}\".")
}

/// `OAuth authentication failed for "<n>".`
#[must_use]
pub fn msg_auth_failed(server_name: &str) -> String {
    format!("OAuth authentication failed for \"{server_name}\".")
}

/// `Failed to authenticate "<n>": <message>` — every aggregate in this module surfaces through
/// here, which is why collapsing an aggregate to its primary error would make the secondary
/// permanently invisible (MCP-345).
#[must_use]
pub fn msg_auth_threw(server_name: &str, message: &str) -> String {
    format!("Failed to authenticate \"{server_name}\": {message}")
}

/// The proxy-tier auth-required text. Overridable by `settings.authRequiredMessage`, which
/// templates `${server}`.
#[must_use]
pub fn msg_auth_required_proxy(server_name: &str) -> String {
    format!(
        "Server \"{server_name}\" requires OAuth authentication. Run mcp({{ action: \"auth-start\", server: \"{server_name}\" }}) to get a browser URL, or /mcp-auth {server_name} in an interactive local session."
    )
}

/// The direct-tools auth-required text — the same sentence with an `MCP server` prefix, and a
/// genuinely different literal from [`msg_auth_required_proxy`].
#[must_use]
pub fn msg_auth_required_direct_tools(server_name: &str) -> String {
    format!(
        "MCP server \"{server_name}\" requires OAuth authentication. Run mcp({{ action: \"auth-start\", server: \"{server_name}\" }}) to get a browser URL, or /mcp-auth {server_name} in an interactive local session."
    )
}

/// `getAuthFailedMessage(...)` — the failure text plus the auth-required guidance.
#[must_use]
pub fn msg_auto_auth_failed(server_name: &str, message: &str) -> String {
    format!(
        "OAuth authentication failed for \"{server_name}\": {message}. Run mcp({{ action: \"auth-start\", server: \"{server_name}\" }}) to get a browser URL, or /mcp-auth {server_name} in an interactive local session."
    )
}

/// `getRedirectPort(authorizationUrl)` (`proxy-modes.ts:94`) — re-parse `redirect_uri` out of the
/// authorization URL and take its port.
#[must_use]
pub fn redirect_port_of(authorization_url: &str) -> Option<u16> {
    let url = url::Url::parse(authorization_url).ok()?;
    let redirect = url
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .map(|(_, value)| value.into_owned())?;
    url::Url::parse(&redirect).ok()?.port()
}

// **De-duplicated at integration.** `formatManualAuthInstructions` is MCP-167, which
// `13d-mcp-proxy-modes.md` owns together with `executeAuthStart` — its only caller. 13g named it
// too (MCP-335) and this file had a second copy; both rendered identically, because upstream's
// `.filter(Boolean)` strips the `""` separators and the copy here simply never emitted them. The
// surviving one documents that mechanism at the site, so it is the one that keeps a future editor
// from "restoring" the blank lines.
pub use crate::proxy::format_manual_auth_instructions;

/// `HttpAuthProviderState` (`server-manager.ts`) — MCP-333.
///
/// **Explicit OAuth touches the credential store at connect time; implicit OAuth defers until the
/// server proves it needs auth**, so a non-OAuth HTTP server never triggers a keyring read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpAuthProviderState {
    /// [`supports_oauth`] said no.
    Disabled,
    /// `auth` was absent: OAuth is possible but nothing has proven it is needed.
    ImplicitDeferred,
    /// A 401 promoted the deferred state; the connect is retried **once**.
    ImplicitChallenged {
        /// The `WWW-Authenticate` value, when the server sent one.
        challenge: Option<String>,
    },
    /// `auth: "oauth"` or an explicit `oauth` block.
    Explicit,
}

/// The initial state for a server definition — `supportsOAuth ? (definition.auth === undefined ?
/// implicit-deferred : explicit) : disabled`.
#[must_use]
pub fn initial_http_auth_state(definition: &ServerEntry) -> HttpAuthProviderState {
    if !supports_oauth(definition) {
        return HttpAuthProviderState::Disabled;
    }
    if definition.auth.is_none() {
        HttpAuthProviderState::ImplicitDeferred
    } else {
        HttpAuthProviderState::Explicit
    }
}

/// What the connect path should do with a 401 — MCP-333.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnauthorizedAction {
    /// Promote and retry the connect exactly once, seeding discovery with `challenge`.
    RetryOnce(HttpAuthProviderState),
    /// Give up and classify the server `needs-auth`, invalidating the auth-entry cache **once**.
    /// The tool layer turns this into either auto-auth or the auth-required message — **never a
    /// browser window opened from inside a turn** (MCP-316's fence).
    NeedsAuth,
    /// OAuth is not available for this server: a 401 is a hard error.
    HardError,
}

/// Advance the four-state union on a 401.
#[must_use]
pub fn on_unauthorized(
    state: &HttpAuthProviderState,
    challenge: Option<&str>,
) -> UnauthorizedAction {
    match state {
        HttpAuthProviderState::Disabled => UnauthorizedAction::HardError,
        HttpAuthProviderState::ImplicitDeferred => {
            UnauthorizedAction::RetryOnce(HttpAuthProviderState::ImplicitChallenged {
                challenge: challenge.map(str::to_string),
            })
        }
        HttpAuthProviderState::ImplicitChallenged { .. } | HttpAuthProviderState::Explicit => {
            UnauthorizedAction::NeedsAuth
        }
    }
}

// ===================================================================================================
// 13 · Tests — the executable half of MCP-347
// ===================================================================================================

// TODO(MCP-347): the acceptance suite below covers the `critical` and `high` units end to end over
// a real loopback socket. Still outstanding, and all of them need a stub authorization server this
// crate does not have yet: the rmcp conformance suites named in MCP-337 (MCP-310, MCP-311, MCP-317,
// MCP-318, MCP-319, MCP-320), `start_auth`'s five stale-registration variants (MCP-328),
// `authenticate`'s dedup and browser ordering (MCP-330), `complete_auth`'s RFC 9207 retry path
// (MCP-323/MCP-331), and `get_valid_token`'s refresh cases (MCP-324).
#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::io::{Read as _, Write as _};
    use std::net::TcpStream;

    use super::*;
    use crate::config::{OAuthConfig, OAuthSetting};

    fn entry_with_oauth(config: OAuthConfig) -> ServerEntry {
        ServerEntry {
            url: Some("https://api.example.com/mcp".to_string()),
            oauth: Some(OAuthSetting::Config(config)),
            ..ServerEntry::default()
        }
    }

    // --- MCP-342: the three placeholder forms -----------------------------------------------

    #[test]
    fn interpolates_all_three_placeholder_forms_in_order() {
        let lookup = |name: &str| match name {
            "A" => Some("one".to_string()),
            "B" => Some("two".to_string()),
            "C" => Some("three".to_string()),
            _ => None,
        };
        assert_eq!(
            interpolate_env_vars_with("${A}/$env:B/{env:C}", lookup),
            "one/two/three"
        );
        // A missing variable expands to the empty string, in every form.
        assert_eq!(interpolate_env_vars_with("[${Z}][$env:Z][{env:Z}]", lookup), "[][][]");
    }

    #[test]
    fn secret_expression_keeps_a_single_bang_and_strips_a_double() {
        assert_eq!(interpolate_secret_expression("!echo hi"), "!echo hi");
        assert_eq!(interpolate_secret_expression("!!echo hi"), "!echo hi");
    }

    // --- MCP-349: `!command` secrets ---------------------------------------------------------

    #[test]
    fn double_bang_resolves_without_a_subprocess() {
        assert_eq!(
            resolve_command_secret("!!literal", "ctx").unwrap(),
            "!literal"
        );
    }

    #[cfg(unix)]
    #[test]
    fn command_secret_success_and_the_three_reachable_failures() {
        assert_eq!(
            resolve_command_secret("!echo hunter2", "ctx").unwrap(),
            "hunter2"
        );
        assert_eq!(
            resolve_command_secret("!false", "ctx").unwrap_err().to_string(),
            "Failed to resolve ctx: command exited with code 1"
        );
        assert_eq!(
            resolve_command_secret("!true", "ctx").unwrap_err().to_string(),
            "Failed to resolve ctx: command returned empty output"
        );
        // stderr is discarded, so a stderr-only command reads as empty output and the text is
        // nowhere in the message.
        let err = resolve_command_secret("!echo boom 1>&2", "ctx")
            .unwrap_err()
            .to_string();
        assert_eq!(err, "Failed to resolve ctx: command returned empty output");
        assert!(!err.contains("boom"));
    }

    // --- MCP-302 / MCP-332: config validation and the OAuth predicate -------------------------

    #[test]
    fn logo_uri_must_be_an_absolute_http_url() {
        for bad in ["./logo.png", "file:///logo.png"] {
            let entry = entry_with_oauth(OAuthConfig {
                logo_uri: Some(bad.to_string()),
                ..OAuthConfig::default()
            });
            assert_eq!(
                extract_oauth_config(&entry).unwrap_err().to_string(),
                "OAuth logoUri must be an absolute http(s) URL",
                "{bad}"
            );
        }
        let entry = entry_with_oauth(OAuthConfig {
            logo_uri: Some(" https://x/logo.png ".to_string()),
            ..OAuthConfig::default()
        });
        assert_eq!(
            extract_oauth_config(&entry).unwrap().logo_uri.as_deref(),
            Some("https://x/logo.png")
        );
    }

    #[test]
    fn empty_after_trim_is_rejected_field_by_field() {
        /// `(field name, the builder that puts a value in that field, the expected message)`.
        type EmptyCase = (&'static str, fn(String) -> OAuthConfig, &'static str);
        let cases: [EmptyCase; 4] = [
            (
                "redirectUri",
                |value| OAuthConfig {
                    redirect_uri: Some(value),
                    ..OAuthConfig::default()
                },
                "OAuth redirectUri must not be empty",
            ),
            (
                "clientName",
                |value| OAuthConfig {
                    client_name: Some(value),
                    ..OAuthConfig::default()
                },
                "OAuth clientName must not be empty",
            ),
            (
                "clientUri",
                |value| OAuthConfig {
                    client_uri: Some(value),
                    ..OAuthConfig::default()
                },
                "OAuth clientUri must not be empty",
            ),
            (
                "logoUri",
                |value| OAuthConfig {
                    logo_uri: Some(value),
                    ..OAuthConfig::default()
                },
                "OAuth logoUri must not be empty",
            ),
        ];
        for (field, build, message) in cases {
            let entry = entry_with_oauth(build("   ".to_string()));
            assert_eq!(
                extract_oauth_config(&entry).unwrap_err().to_string(),
                message,
                "{field}"
            );
        }
    }

    #[test]
    fn client_secret_command_expressions_survive_uninterpolated() {
        let entry = entry_with_oauth(OAuthConfig {
            client_secret: Some("!op read op://x/y".to_string()),
            ..OAuthConfig::default()
        });
        assert_eq!(
            extract_oauth_config(&entry).unwrap().client_secret.as_deref(),
            Some("!op read op://x/y")
        );
    }

    #[test]
    fn raw_block_validation_emits_the_type_messages() {
        use indexmap::IndexMap;
        let mut block = IndexMap::new();
        block.insert("clientId".to_string(), RawJson::Number(7.into()));
        assert_eq!(
            validate_oauth_block(&RawJson::Object(block)).unwrap_err().to_string(),
            "OAuth clientId must be a string"
        );

        let mut block = IndexMap::new();
        block.insert(
            "authorizationParams".to_string(),
            RawJson::Array(vec![RawJson::Bool(true)]),
        );
        assert_eq!(
            validate_oauth_block(&RawJson::Object(block)).unwrap_err().to_string(),
            "OAuth authorizationParams must be an object"
        );

        let mut params = IndexMap::new();
        params.insert("prompt".to_string(), RawJson::Bool(true));
        let mut block = IndexMap::new();
        block.insert("authorizationParams".to_string(), RawJson::Object(params));
        assert_eq!(
            validate_oauth_block(&RawJson::Object(block)).unwrap_err().to_string(),
            "OAuth authorizationParams.prompt must be a string"
        );

        let mut block = IndexMap::new();
        block.insert(
            "skipIssuerMetadataValidation".to_string(),
            RawJson::String("yes".to_string()),
        );
        assert_eq!(
            validate_oauth_block(&RawJson::Object(block)).unwrap_err().to_string(),
            "OAuth skipIssuerMetadataValidation must be a boolean"
        );

        // `oauth: false` short-circuits before any field is read.
        assert!(validate_oauth_block(&RawJson::Bool(false)).is_ok());
    }

    #[test]
    fn supports_oauth_truth_table_in_order() {
        let http = |entry: ServerEntry| entry;

        // No URL.
        assert!(!supports_oauth(&ServerEntry::default()));

        let base = ServerEntry {
            url: Some("https://x/mcp".to_string()),
            ..ServerEntry::default()
        };
        // auth === false
        assert!(!supports_oauth(&http(ServerEntry {
            auth: Some(AuthMode::Disabled(false)),
            ..base.clone()
        })));
        // oauth === false
        assert!(!supports_oauth(&http(ServerEntry {
            oauth: Some(OAuthSetting::Disabled(false)),
            ..base.clone()
        })));
        // auth === "oauth" BEATS custom headers.
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("x-tenant".to_string(), "acme".to_string());
        assert!(supports_oauth(&http(ServerEntry {
            auth: Some(AuthMode::Named(AuthKind::Oauth)),
            headers: Some(headers.clone().into()),
            ..base.clone()
        })));
        // headers alone disable auto-detection.
        assert!(!supports_oauth(&http(ServerEntry {
            headers: Some(headers.into()),
            ..base.clone()
        })));
        // auth === undefined
        assert!(supports_oauth(&base));
        // auth === "bearer"
        assert!(!supports_oauth(&http(ServerEntry {
            auth: Some(AuthMode::Named(AuthKind::Bearer)),
            ..base
        })));
    }

    // --- MCP-303: loopback-only redirect validation -------------------------------------------

    #[test]
    fn redirect_uri_checks_run_in_upstream_order() {
        assert_eq!(
            parse_oauth_redirect_uri("not a url").unwrap_err().to_string(),
            "Invalid OAuth redirectUri: not a url"
        );
        // A non-loopback URL that ALSO carries a fragment reports the loopback error.
        assert_eq!(
            parse_oauth_redirect_uri("http://example.com:8080/cb#frag")
                .unwrap_err()
                .to_string(),
            "OAuth redirectUri must be an http:// localhost or loopback URI"
        );
        // Narrower than RFC 8252 §7.3 — only the four literals pass.
        assert_eq!(
            parse_oauth_redirect_uri("http://127.0.0.2:8080/cb")
                .unwrap_err()
                .to_string(),
            "OAuth redirectUri must be an http:// localhost or loopback URI"
        );
        assert_eq!(
            parse_oauth_redirect_uri("https://localhost:8080/cb")
                .unwrap_err()
                .to_string(),
            "OAuth redirectUri must be an http:// localhost or loopback URI"
        );
        assert_eq!(
            parse_oauth_redirect_uri("http://user:pw@localhost:8080/cb")
                .unwrap_err()
                .to_string(),
            "OAuth redirectUri must not include username or password"
        );
        assert_eq!(
            parse_oauth_redirect_uri("http://localhost:8080/cb#frag")
                .unwrap_err()
                .to_string(),
            "OAuth redirectUri must not include a fragment"
        );
        assert_eq!(
            parse_oauth_redirect_uri("http://localhost/cb")
                .unwrap_err()
                .to_string(),
            "OAuth redirectUri must include an explicit numeric port"
        );

        let parsed = parse_oauth_redirect_uri("http://[::1]:8080/cb").unwrap();
        assert_eq!(
            parsed,
            RedirectEndpoint {
                port: 8080,
                callback_host: "::1".to_string(),
                callback_path: "/cb".to_string(),
            }
        );
    }

    // --- MCP-307: the three pages --------------------------------------------------------------

    #[test]
    fn pages_are_self_contained_and_host_branded() {
        set_app_name(Some("arc".to_string()));
        let success = html_success();
        let manual = html_manual_success();
        let error = html_error("<script>alert(1)</script>");
        set_app_name(None);

        for page in [&success, &manual, &error] {
            // Zero absolute URLs: the icons carry no `xmlns`.
            assert!(!page.contains("http://"), "{page}");
            assert!(!page.contains("https://"), "{page}");
            assert!(page.contains("arc"), "host branding");
        }
        assert!(success.contains("arc — Authorization Successful"));
        assert!(success.contains("setTimeout(() => window.close(), 2000)"));
        assert!(!manual.contains("<script>"));
        assert!(manual.contains("arc — Authorization Received"));
        assert!(error.contains("arc — Authorization Failed"));
        // The served HTML escapes the provider text…
        assert!(error.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!error.contains("<script>alert(1)</script>"));
    }

    #[test]
    fn escape_html_order_does_not_double_escape() {
        assert_eq!(escape_html("&<>\"'"), "&amp;&lt;&gt;&quot;&#39;");
    }

    // --- MCP-316: the reserved-parameter guard --------------------------------------------------

    #[test]
    fn reserved_and_already_present_authorization_params_are_refused() {
        let url = "https://as.example.com/authorize?response_type=code&client_id=abc&state=xyz&code_challenge=c&code_challenge_method=S256&redirect_uri=http%3A%2F%2Flocalhost%3A1%2Fcb&scope=a&resource=https%3A%2F%2Fx";
        for key in RESERVED_AUTHORIZATION_PARAMS {
            let mut params = std::collections::BTreeMap::new();
            params.insert(key.to_string(), "hijack".to_string());
            assert_eq!(
                add_authorization_params(url, Some(&params))
                    .unwrap_err()
                    .to_string(),
                format!(
                    "OAuth authorizationParams.{key} cannot override an authorization flow parameter"
                )
            );
        }

        // A non-reserved key that is already on the URL is refused too — this half does all the
        // work, because rmcp builds the URL and the guard decorates it afterwards.
        let already = "https://as.example.com/authorize?audience=one";
        let mut params = std::collections::BTreeMap::new();
        params.insert("audience".to_string(), "two".to_string());
        assert!(add_authorization_params(already, Some(&params)).is_err());

        // A fresh key is set. `prompt` is deliberately not reserved.
        let mut params = std::collections::BTreeMap::new();
        params.insert("prompt".to_string(), "consent".to_string());
        let decorated = add_authorization_params(url, Some(&params)).unwrap();
        assert!(decorated.contains("prompt=consent"));
        assert_eq!(csrf_token_of(&decorated).as_deref(), Some("xyz"));
    }

    // --- MCP-326: the manual/headless parser ----------------------------------------------------

    #[test]
    fn parses_urls_hash_merges_bare_queries_and_bare_codes() {
        let from_url = parse_authorization_redirect_input(
            "http://localhost:19876/callback?code=abc&state=s1&iss=https%3A%2F%2Fas",
            Some("s1"),
        )
        .unwrap();
        assert_eq!(from_url.code, "abc");
        assert_eq!(from_url.iss.as_deref(), Some("https://as"));

        // Fragment parameters are merged when the query does not already carry them.
        let merged =
            parse_authorization_redirect_input("http://localhost:1/cb?state=s1#code=frag", Some("s1"))
                .unwrap();
        assert_eq!(merged.code, "frag");

        // A bare query string is accepted only when it looks like a callback.
        assert_eq!(
            parse_authorization_redirect_input("?code=xyz&state=s1", Some("s1"))
                .unwrap()
                .code,
            "xyz"
        );
        // A bare code.
        assert_eq!(
            parse_authorization_redirect_input("A1b2-c3_d4~e5.f6", None).unwrap().code,
            "A1b2-c3_d4~e5.f6"
        );

        assert_eq!(
            parse_authorization_redirect_input("   ", None).unwrap_err().to_string(),
            "Authorization code or redirect URL is required"
        );
        assert_eq!(
            parse_authorization_redirect_input("?error=access_denied", None)
                .unwrap_err()
                .to_string(),
            "access_denied"
        );
        assert_eq!(
            parse_authorization_redirect_input("?error=access_denied&error_description=nope", None)
                .unwrap_err()
                .to_string(),
            "access_denied: nope"
        );
        assert_eq!(
            parse_authorization_redirect_input("?code=abc", Some("s1"))
                .unwrap_err()
                .to_string(),
            "OAuth state missing from redirect URL"
        );
        assert_eq!(
            parse_authorization_redirect_input("?code=abc&state=other", Some("s1"))
                .unwrap_err()
                .to_string(),
            "OAuth state mismatch - potential CSRF attack"
        );
        assert_eq!(
            parse_authorization_redirect_input("not a code!!", None)
                .unwrap_err()
                .to_string(),
            "Could not find an OAuth authorization code in the provided input"
        );
    }

    // --- MCP-315: the expiry tri-state ----------------------------------------------------------

    #[test]
    fn expiry_is_tri_state() {
        assert_eq!(is_token_expired(None, 1_000), None);
        let no_expiry = McpTokens {
            access_token: "a".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: None,
            issuer: None,
        };
        assert_eq!(is_token_expired(Some(&no_expiry), 1_000), Some(false));
        let future = McpTokens {
            expires_at: Some(2_000),
            ..no_expiry.clone()
        };
        assert_eq!(is_token_expired(Some(&future), 1_000), Some(false));
        let past = McpTokens {
            expires_at: Some(500),
            ..no_expiry
        };
        assert_eq!(is_token_expired(Some(&past), 1_000), Some(true));
    }

    #[test]
    fn a_client_secret_expiry_of_zero_never_expires() {
        let mut record = StoredClientInfo::new("cid");
        record.client_secret_expires_at = Some(0.0);
        assert!(!client_secret_expired(&record, now_secs() as f64));
        record.client_secret_expires_at = Some(1.0);
        assert!(client_secret_expired(&record, now_secs() as f64));
    }

    // --- MCP-335: the six-line instruction block ------------------------------------------------

    #[test]
    fn manual_auth_instructions_have_six_lines_and_no_interior_blanks() {
        let block = format_manual_auth_instructions("acme", "https://as/authorize?client_id=x");
        let lines: Vec<&str> = block.split('\n').collect();
        assert_eq!(lines.len(), 6, "{block}");
        assert!(lines.iter().all(|line| !line.is_empty()));
        assert_eq!(lines[0], "MCP OAuth required for \"acme\".");

        let with_port = format_manual_auth_instructions(
            "acme",
            "https://as/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A19876%2Fcallback",
        );
        let lines: Vec<&str> = with_port.split('\n').collect();
        // Six content lines, one blank, then the note.
        assert_eq!(lines.len(), 8, "{with_port}");
        assert_eq!(lines[6], "");
        assert!(lines[7].contains("local port 19876"));
        assert_eq!(
            redirect_port_of("https://as/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A19876%2Fcallback"),
            Some(19876)
        );
    }

    // --- MCP-333: the connect-path classification -----------------------------------------------

    #[test]
    fn implicit_oauth_retries_once_then_needs_auth() {
        let implicit = ServerEntry {
            url: Some("https://x/mcp".to_string()),
            ..ServerEntry::default()
        };
        let state = initial_http_auth_state(&implicit);
        assert_eq!(state, HttpAuthProviderState::ImplicitDeferred);
        let promoted = match on_unauthorized(&state, Some("Bearer realm=\"x\"")) {
            UnauthorizedAction::RetryOnce(next) => next,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            promoted,
            HttpAuthProviderState::ImplicitChallenged {
                challenge: Some("Bearer realm=\"x\"".to_string())
            }
        );
        assert_eq!(
            on_unauthorized(&promoted, None),
            UnauthorizedAction::NeedsAuth
        );

        let explicit = ServerEntry {
            auth: Some(AuthMode::Named(AuthKind::Oauth)),
            ..implicit.clone()
        };
        assert_eq!(
            initial_http_auth_state(&explicit),
            HttpAuthProviderState::Explicit
        );
        assert_eq!(
            on_unauthorized(&HttpAuthProviderState::Explicit, None),
            UnauthorizedAction::NeedsAuth
        );

        let bearer = ServerEntry {
            auth: Some(AuthMode::Named(AuthKind::Bearer)),
            ..implicit
        };
        assert_eq!(
            initial_http_auth_state(&bearer),
            HttpAuthProviderState::Disabled
        );
        assert_eq!(
            on_unauthorized(&HttpAuthProviderState::Disabled, None),
            UnauthorizedAction::HardError
        );
    }

    // --- MCP-301 / MCP-344: runtime isolation and the live set -----------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutting_one_runtime_down_leaves_the_other_alone() {
        let first = create_oauth_runtime(None);
        let second = create_oauth_runtime(None);
        assert!(!first.token().is_cancelled());

        shutdown_oauth(&first).await;
        assert!(first.token().is_cancelled());
        assert!(!second.token().is_cancelled());
        assert_eq!(
            first.throw_if_aborted().unwrap_err().to_string(),
            OAUTH_RUNTIME_STOPPED
        );
        // A second shutdown is a no-op rather than a double generation bump.
        let generation = first.generation();
        shutdown_oauth(&first).await;
        assert_eq!(first.generation(), generation);

        shutdown_oauth(&second).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_pending_auth_refuses_across_a_generation_bump() {
        let runtime = create_oauth_runtime(None);
        let storage: Arc<dyn McpOAuthStorage> =
            Arc::new(InMemoryOAuthStorage::new(PathBuf::from("/base")));
        let stale = runtime.generation();
        runtime.generation.fetch_add(1, Ordering::SeqCst);

        let pending = PendingAuth {
            server_name: "acme".to_string(),
            server_url: "https://x/mcp".to_string(),
            authorization_url: "https://as/authorize".to_string(),
            session: Arc::new(AsyncMutex::new(None)),
            expected_issuer: None,
            requires_issuer: false,
            storage: Arc::clone(&storage),
            base_dir: storage.base_dir(),
        };
        let error = set_pending_auth(&runtime, pending, "state", stale)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), OAUTH_RUNTIME_STOPPED);
        shutdown_oauth(&runtime).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stale_timer_state_guard_spares_a_newer_flow() {
        let runtime = create_oauth_runtime(None);
        let storage: Arc<dyn McpOAuthStorage> =
            Arc::new(InMemoryOAuthStorage::new(PathBuf::from("/base")));
        let base_dir = storage.base_dir();
        let make = |url: &str| PendingAuth {
            server_name: "acme".to_string(),
            server_url: url.to_string(),
            authorization_url: "https://as/authorize".to_string(),
            session: Arc::new(AsyncMutex::new(None)),
            expected_issuer: None,
            requires_issuer: false,
            storage: Arc::clone(&storage),
            base_dir: base_dir.clone(),
        };

        let generation = runtime.generation();
        set_pending_auth(&runtime, make("https://a"), "state-a", generation)
            .await
            .unwrap();
        set_pending_auth(&runtime, make("https://b"), "state-b", generation)
            .await
            .unwrap();

        // The old flow's timer fires with the OLD state; the newer flow must survive.
        clear_pending_auth(&runtime, "acme", Some("state-a"), &base_dir, None)
            .await
            .unwrap();
        assert!(has_pending_auth(&runtime, "acme", Some(&base_dir)).await);

        clear_pending_auth(&runtime, "acme", Some("state-b"), &base_dir, None)
            .await
            .unwrap();
        assert!(!has_pending_auth(&runtime, "acme", None).await);
        shutdown_oauth(&runtime).await;
    }

    // --- MCP-305 / MCP-306 / MCP-308: the listener, end to end over a real socket -----------------

    fn request(port: u16, target: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            stream,
            "GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        // Half-close so the listener's trailing "let the client read the response" read returns
        // immediately instead of sitting on its 10-second timeout.
        let _ = stream.shutdown(std::net::Shutdown::Write);
        let mut raw = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            // A reset after the response has been written is normal on macOS; whatever was read
            // before it is the response.
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => raw.extend_from_slice(&chunk[..read]),
            }
        }
        let text = String::from_utf8_lossy(&raw).into_owned();
        let status = text
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse::<u16>().ok())
            .unwrap_or(0);
        (status, text)
    }

    async fn get(port: u16, target: &str) -> (u16, String) {
        let target = target.to_string();
        tokio::task::spawn_blocking(move || request(port, &target))
            .await
            .unwrap()
    }

    /// The whole listener surface in one test: the process-global runtime makes parallel listener
    /// tests race, so MCP-305, MCP-306 and MCP-308 are driven as one ordered sequence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_callback_listener_end_to_end() {
        // The listener is process-shared and `shutdown_oauth` stops it when the live-runtime set
        // empties (MCP-344) — which is exactly what the runtime tests running in parallel do. A
        // live runtime held for the duration keeps the set non-empty, which is the same guarantee
        // two concurrent sessions rely on.
        let guard = create_oauth_runtime(None);

        ensure_callback_server(EnsureCallbackServer {
            strict_port: false,
            oauth_state: Some("reserved-1".to_string()),
            reserve_state: true,
            ..EnsureCallbackServer::default()
        })
        .await
        .unwrap();
        assert!(is_callback_server_running().await);
        let (port, path) = callback_endpoint().await;
        assert_eq!(path, DEFAULT_OAUTH_CALLBACK_PATH);
        // The default (dynamic-registration) flow never binds 19876.
        assert_ne!(port, DEFAULT_OAUTH_CALLBACK_PORT);
        assert!(
            callback_redirect_uri()
                .await
                .unwrap()
                .starts_with(&format!("http://localhost:{port}"))
        );

        // Branch 2 — no state.
        let (status, body) = get(port, "/callback?code=abc").await;
        assert_eq!(status, 400);
        assert!(body.contains("Missing required state parameter - potential CSRF attack"));

        // Branch 5 — unknown state.
        let (status, body) = get(port, "/callback?code=abc&state=nope").await;
        assert_eq!(status, 400);
        assert!(body.contains("Invalid or expired state parameter"));

        // Branch 3 — an error for an unknown state does NOT reflect error_description.
        let (status, body) =
            get(port, "/callback?state=nope&error=bad&error_description=REFLECTED").await;
        assert_eq!(status, 400);
        assert!(!body.contains("REFLECTED"));
        assert!(body.contains("Invalid or expired state parameter"));

        // Branch 6 — a known state with no code leaves the waiter pending.
        let (status, body) = get(port, "/callback?state=reserved-1").await;
        assert_eq!(status, 400);
        assert!(body.contains("No authorization code provided"));

        // Branch 7 — reserved but not awaited: the manual page, reservation intact.
        let (status, body) = get(port, "/callback?state=reserved-1&code=abc").await;
        assert_eq!(status, 200);
        assert!(body.contains("Authorization Received"));

        // Branch 4 against a merely reserved state: the reservation SURVIVES.
        let (status, body) =
            get(port, "/callback?state=reserved-1&error=access_denied").await;
        assert_eq!(status, 200);
        assert!(body.contains("access_denied"));
        let (status, _) = get(port, "/callback?state=reserved-1&code=abc").await;
        assert_eq!(status, 200);

        // MCP-305 — a reservation blocks a strict endpoint switch.
        let refused = ensure_callback_server(EnsureCallbackServer {
            strict_port: true,
            port: Some(19999),
            oauth_state: Some("reserved-2".to_string()),
            reserve_state: true,
            ..EnsureCallbackServer::default()
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(
            refused.contains("cannot be switched while authorizations are pending"),
            "{refused}"
        );
        // …and a path switch.
        let refused = ensure_callback_server(EnsureCallbackServer {
            callback_path: Some("/other".to_string()),
            ..EnsureCallbackServer::default()
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(refused.contains("but callback path /other is required"), "{refused}");

        // MCP-308 — promote the reservation, then branch 8.
        assert_eq!(pending_callback_count(), 0);
        let waiter = wait_for_callback("reserved-1");
        assert_eq!(pending_callback_count(), 1);
        let (status, body) = get(port, "/callback?state=reserved-1&code=THE-CODE&iss=https%3A%2F%2Fas").await;
        assert_eq!(status, 200);
        assert!(body.contains("Authorization Successful"));
        assert!(body.contains("window.close()"));
        let settled = waiter.await.unwrap().unwrap();
        assert_eq!(settled.code, "THE-CODE");
        assert_eq!(settled.iss.as_deref(), Some("https://as"));
        assert_eq!(pending_callback_count(), 0);

        // `cancelPendingCallback` rejects with its own message.
        let waiter = wait_for_callback("cancel-me");
        cancel_pending_callback("cancel-me");
        assert_eq!(
            waiter.await.unwrap().unwrap_err().to_string(),
            "Authorization cancelled"
        );

        // Stopping rejects every waiter and resets the endpoint…
        let a = wait_for_callback("stop-a");
        let b = wait_for_callback("stop-b");
        stop_callback_server().await;
        assert_eq!(a.await.unwrap().unwrap_err().to_string(), CALLBACK_STOPPED);
        assert_eq!(b.await.unwrap().unwrap_err().to_string(), CALLBACK_STOPPED);
        assert!(!is_callback_server_running().await);
        let (reset_port, reset_path) = callback_endpoint().await;
        assert_eq!(reset_port, configured_callback_port());
        assert_eq!(reset_path, DEFAULT_OAUTH_CALLBACK_PATH);

        // …and the listener is RESTARTABLE afterwards: a `OnceCell` gate would wedge here.
        ensure_callback_server(EnsureCallbackServer::default())
            .await
            .unwrap();
        assert!(is_callback_server_running().await);
        stop_callback_server().await;

        shutdown_oauth(&guard).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reservation_requires_a_state() {
        let error = ensure_callback_server(EnsureCallbackServer {
            reserve_state: true,
            ..EnsureCallbackServer::default()
        })
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "OAuth callback reservation requires an oauthState"
        );
    }

    // --- MCP-345: both errors survive a failing cleanup -------------------------------------------

    #[test]
    fn an_aggregate_renders_both_child_messages_and_drops_the_phase() {
        let aggregate = aggregate_error(
            PHASE_STARTUP_CLEANUP,
            McpError::other("port in use"),
            McpError::other("keychain locked"),
        );
        // MEASURED, node 22, upstream's own `formatTerminalError` on
        // `new AggregateError([Error("port in use"), Error("keychain locked")],
        //   "OAuth startup cleanup failed")` → `"port in use: keychain locked"`.
        // The phase is the aggregate's `.message`, and `formatTerminalError` pushes `.message`
        // only when no child contributed text (`utils.ts:249`). This test used to assert the
        // phase-prefixed form; it was pinning a divergence.
        assert_eq!(aggregate.to_string(), "port in use: keychain locked");
        assert_eq!(aggregate.aggregate_head(), Some(PHASE_STARTUP_CLEANUP));
        // The structure survives, not just the rendering.
        assert!(matches!(
            &aggregate,
            McpError::OAuthAggregate { phase, errors }
                if *phase == PHASE_STARTUP_CLEANUP && errors.len() == 2
        ));
        assert!(aggregate.is_cleanup_failure());
    }

    /// A credential-store failure during cleanup must stay *classifiable* through the aggregate.
    /// Section 07's refresh driver rethrows the store class and swallows everything else into
    /// `None`; if the aggregate flattened it, a broken keychain would become an infinite silent
    /// re-auth loop.
    #[test]
    fn a_store_failure_inside_an_aggregate_is_still_a_store_failure() {
        let store_error = crate::credentials::AuthStoreError::MissingChunk {
            chunk_account: "sha256-aa.chunk.bb.1".to_string(),
            server: "linear".to_string(),
        };
        let aggregate = aggregate_error(
            PHASE_CANCELLATION_CLEANUP,
            McpError::other("user cancelled"),
            McpError::CredentialStore(store_error),
        );
        let McpError::OAuthAggregate { errors, .. } = &aggregate else {
            panic!("expected an aggregate");
        };
        assert!(
            errors.iter().any(McpError::is_credential_store_failure),
            "the store class must survive the aggregate: {aggregate}"
        );
    }

    // --- MCP-321: the URL binding ------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn credentials_are_bound_to_the_exact_server_url() {
        let storage: Arc<dyn McpOAuthStorage> =
            Arc::new(InMemoryOAuthStorage::new(PathBuf::from("/base")));
        storage
            .save_client(
                "acme",
                "https://x/mcp",
                Some(StoredClientInfo::new("cid")),
            )
            .await
            .unwrap();

        assert!(
            storage
                .get_auth_for_url("acme", "https://x/mcp")
                .await
                .unwrap()
                .is_some()
        );
        // A trailing slash is a different URL: no normalization, ever.
        assert!(
            storage
                .get_auth_for_url("acme", "https://x/mcp/")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            storage
                .get_auth_for_url("acme", "https://y/mcp")
                .await
                .unwrap()
                .is_none()
        );
    }

    // --- MCP-304: the env override is read once and validated ---------------------------------------

    #[test]
    fn the_configured_port_defaults_when_the_env_is_unusable() {
        // The value is a process-wide `OnceLock`, so this asserts the default rather than
        // mutating the environment out from under a parallel test.
        let port = configured_callback_port();
        assert!(port > 0);
        assert_eq!(port, configured_callback_port());
    }
}
