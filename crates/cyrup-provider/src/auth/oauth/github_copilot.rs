//! GitHub Copilot's OAuth **login** flow — port of pi v0.83.0
//! `packages/ai/src/auth/oauth/github-copilot.ts` (379 lines).
//!
//! This is the half `crates/cyrup-provider/src/providers/github_copilot.rs` explicitly left out
//! (see its module doc, "What is not here"): the GitHub device-code grant, the post-login
//! policy-acceptance pass, and the interactive GitHub-Enterprise-domain prompt. The runtime half
//! — `refresh` and `toAuth` — already lives in that provider module as
//! [`crate::providers::github_copilot::GitHubCopilotOAuth`], and this flow **delegates** to it
//! rather than restating it, so `githubCopilotOAuth` (`github-copilot.ts:367-379`) is served by
//! exactly one implementation of each member.
//!
//! ## Provenance
//!
//! | this module | `github-copilot.ts` |
//! |---|---|
//! | [`CLIENT_ID`] | `:9-10` |
//! | [`GitHubDeviceUrls`] / [`GitHubCopilotLogin::urls`] | `getUrls`, `:51-61` |
//! | [`parse_http_href`] | `new URL(verificationUri)` + protocol guard, `:174-184` |
//! | [`GitHubCopilotLogin::fetch_json`] | `fetchJson`, `:126-137` |
//! | [`GitHubCopilotLogin::start_device_flow`] | `startDeviceFlow`, `:139-193` |
//! | [`AccessTokenPoller`] | the `poll` closure of `pollForGitHubAccessToken`, `:195-242` |
//! | [`GitHubCopilotLogin::exchange_copilot_token`] | `refreshGitHubCopilotAccessToken`, `:244-277` |
//! | [`GitHubCopilotLogin::fetch_with_rate_limit_retry`] | `fetchWithRateLimitRetry`, `:135-166` |
//! | [`GitHubCopilotLogin::fetch_model_catalog`] | `fetchGitHubCopilotModels`, `:168-194` |
//! | [`GitHubCopilotLogin::enable_model`] | `enableGitHubCopilotModel`, `:373-408` |
//! | [`GitHubCopilotLogin::enable_models`] | `enableGitHubCopilotModels`, `:414-432` |
//! | [`GitHubCopilotLogin::run_login`] | `loginGitHubCopilot`, `:434-485` |
//! | `impl OAuthAuth for GitHubCopilotLogin` | `githubCopilotOAuth`, `:367-379` |
//!
//! `normalizeDomain` (`:40-49`), `getBaseUrlFromToken` (`:63-73`), `getGitHubCopilotBaseUrl`
//! (`:75-85`), `parseGitHubCopilotModelCatalog` (`:93-132`), `COPILOT_HEADERS` (`:12-17`) and
//! `COPILOT_API_VERSION` (`:18`) were ported with the provider and are imported from
//! [`crate::providers::github_copilot`] unchanged. (`isSelectableCopilotModel` no longer exists
//! upstream: v0.87.1 inlined its three predicates into `parseGitHubCopilotModelCatalog`.)
//!
//! ## Mechanism divergences (Rust forces these; behaviour is unchanged)
//!
//! * **No ambient `atob`.** Upstream obfuscates the (public, non-secret) client id as base64 and
//!   decodes it at load time (`:9-10`). Decoding a compile-time-known constant at runtime would be
//!   a fallible operation with no failure handling, so [`CLIENT_ID`] is the decoded literal;
//!   `client_id_matches_upstream_base64` asserts the two agree against the upstream base64.
//! * **No ambient `fetch`/`URL`.** Requests go through `reqwest` built by
//!   [`crate::stream::sse::build_client_for_target`] so the two GitHub origins honour the same
//!   `HTTP(S)_PROXY`/`NO_PROXY` policy as provider traffic; `new URL(...).href` is
//!   [`parse_http_href`], which implements the subset of WHATWG normalization these endpoints can
//!   produce (see its doc).
//! * **`AbortSignal` is [`CancelToken`]** (arch-00 §3.2). `interaction.signal?.aborted` (`:335`)
//!   is [`AuthInteraction::cancel`] + [`CancelToken::is_cancelled`].
//! * **Errors are typed.** Upstream `throw new Error(msg)`; here the same message is the `Display`
//!   of an [`OAuthError`], folded into the crate taxonomy by
//!   [`OAuthError::into_auth_error`] at the trait boundary (func-01 R-01-017).
//! * **The policy pass is SEQUENTIAL** ([`GitHubCopilotLogin::enable_models`], a plain `for` loop),
//!   because `enableGitHubCopilotModels` (`:414-432` @v0.87.1) is one: it collects the successful ids
//!   and `break`s the batch on an exhausted rate limit, neither of which a concurrent fan-out can
//!   express. This supersedes the earlier note here, which recorded `join_all` as the port of
//!   v0.83.0's `Promise.all` over the whole embedded catalog (PROV-098).
//! * **Endpoint overrides.** Upstream's tests stub the ambient `fetch`. Rust has none, so the
//!   three origins are `#[cfg(test)]`-settable struct fields; production always derives them from
//!   the domain and the Copilot token's `proxy-ep=` claim.
//! * **Duplicated token exchange.** [`GitHubCopilotLogin::exchange_copilot_token`] restates
//!   `refreshGitHubCopilotAccessToken` (`:244-277`) even though
//!   [`crate::providers::github_copilot::GitHubCopilotOAuth`] already ports it, because that copy
//!   is a private method and because `loginGitHubCopilot` needs the exchange **without** the model
//!   listing that `refreshGitHubCopilotToken` (`:282-288`) bolts on — the listing has to happen
//!   *after* the policy pass (`:352-357`). Calling the public `refresh` instead would issue an
//!   extra `/models` request before enabling anything, which is not what upstream does.

use super::OAuthError;
use super::device_code::{
    DeviceCodePollOptions, DeviceCodePollResult, DeviceCodePoller, poll_oauth_device_code_flow,
};
use super::interaction::{AuthEvent, AuthInteraction, AuthPrompt};
use super::query::encode_query;
use crate::auth::types::{AuthContext, Credential, EnvAuthContext, ModelAuth};
use crate::auth::{LoginOptions, OAuthAuth};
use crate::error::AuthError;
use crate::providers::github_copilot::{
    COPILOT_API_VERSION, COPILOT_HEADERS, CopilotModelCatalog, DEFAULT_GITHUB_DOMAIN,
    GITHUB_COPILOT_BASE_URL, GITHUB_COPILOT_PROVIDER_ID, GitHubCopilotOAuth,
    github_copilot_base_url, normalize_domain, parse_github_copilot_model_catalog,
};
use crate::utils::http_date::parse_http_date_ms;
use crate::utils::provider_plumbing::now_millis;
use crate::utils::provider_retry::js_parse_float;
use cyrup_core::CancelToken;
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Constants — github-copilot.ts:9-18, :150, :332-333, :353
// ---------------------------------------------------------------------------

/// The public GitHub OAuth app id. Upstream writes it as `atob("SXYxLmI1MDdhMDhjODdlY2ZlOTg=")`
/// (`github-copilot.ts:9-10`); see the module doc for why the decoded literal is the constant.
pub const CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// `github-copilot.ts:10` — the exact base64 upstream decodes, kept so the equality test asserts
/// against an upstream fixture rather than restating [`CLIENT_ID`].
#[cfg(test)]
const CLIENT_ID_BASE64: &str = "SXYxLmI1MDdhMDhjODdlY2ZlOTg=";

/// `github-copilot.ts:150` — the only scope the device grant asks for.
pub const DEVICE_CODE_SCOPE: &str = "read:user";

/// `github-copilot.ts:217` — RFC 8628 §3.4.
pub const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// The `User-Agent` the two `login/…` endpoints are called with (`github-copilot.ts:146`, `:212`).
/// It is the same value as `COPILOT_HEADERS["User-Agent"]`, but upstream sends *only* this header
/// on the GitHub device endpoints and the full editor-identity set on the Copilot endpoints.
const DEVICE_FLOW_USER_AGENT: &str = "GitHubCopilotChat/0.35.0";

/// `github-copilot.ts:332`.
const ENTERPRISE_PROMPT_MESSAGE: &str = "GitHub Enterprise URL/domain (blank for github.com)";
/// `github-copilot.ts:333`.
const ENTERPRISE_PROMPT_PLACEHOLDER: &str = "company.ghe.com";
/// `github-copilot.ts:353`.
const ENABLING_MODELS_MESSAGE: &str = "Enabling models...";

/// `github-copilot.ts:339`.
const INVALID_ENTERPRISE_DOMAIN: &str = "Invalid GitHub Enterprise URL/domain";
/// `github-copilot.ts:155`.
const INVALID_DEVICE_CODE_RESPONSE: &str = "Invalid device code response";
/// `github-copilot.ts:171`.
const INVALID_DEVICE_CODE_FIELDS: &str = "Invalid device code response fields";
/// `github-copilot.ts:180`, `:183`.
const UNTRUSTED_VERIFICATION_URI: &str = "Untrusted verification_uri in device code response";
/// `github-copilot.ts:239`.
const INVALID_DEVICE_TOKEN_RESPONSE: &str = "Invalid device token response";
/// `github-copilot.ts:260`.
const INVALID_COPILOT_TOKEN_RESPONSE: &str = "Invalid Copilot token response";
/// `github-copilot.ts:267`.
const INVALID_COPILOT_TOKEN_FIELDS: &str = "Invalid Copilot token response fields";

/// `AbortSignal.timeout(5000)` — the cap pi puts on EVERY attempt issued through
/// `fetchWithRateLimitRetry`, not just on the model listing (`github-copilot.ts:150` @v0.87.1,
/// inside the retry loop: `signal: AbortSignal.any([requestSignal, AbortSignal.timeout(5000)])`).
/// So the listing GET and each `/models/{id}/policy` POST are bounded alike. PROV-098 corrected both
/// the name and the citation, which used to read `:123` and "on the model listing".
const COPILOT_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(5000);

/// pi's `{ maxRetries: 2, maxElapsedMs: 5000 }` — the retry policy `loginGitHubCopilot` passes to
/// `fetchGitHubCopilotModels` (`github-copilot.ts:462-470`) and `enableGitHubCopilotModel` passes to
/// `fetchWithRateLimitRetry` (`:398`). PROV-098.
const COPILOT_MAX_RATE_LIMIT_RETRIES: u32 = 2;
const COPILOT_MAX_RATE_LIMIT_ELAPSED_MS: i64 = 5000;

/// One attempt was aborted before it could produce a response — either the per-attempt
/// `AbortSignal.timeout(5000)` (`github-copilot.ts:150`) or the armed retry budget
/// `AbortSignal.timeout(maxElapsedMs)` (`:143`) fired.
///
/// In pi both arrive as an abort on the COMPOSED signal, so the caller's own `signal.aborted` is
/// still `false`: `enableGitHubCopilotModel`'s `catch` (`:400-403`) turns it into `false` and the
/// batch carries on, while a real caller cancellation is rethrown. That is why this is an
/// `OAuthError::Failed` and never an `OAuthError::Cancelled`.
const COPILOT_ATTEMPT_ABORTED: &str = "GitHub Copilot request timed out";

/// pi's `retryPolicy` parameter (`github-copilot.ts:139` @v0.87.1) plus the per-attempt cap pi
/// hardcodes one line into the loop body (`:150`).
///
/// The cap is a FIELD rather than a constant read inside the loop for one reason: a test can then
/// shorten it to milliseconds and assert the bound without spending five real seconds. Production
/// only ever uses [`CopilotRetryPolicy::LOGIN`], which carries pi's literals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CopilotRetryPolicy {
    /// `retryPolicy.maxRetries` (`:139`).
    max_retries: u32,
    /// `retryPolicy.maxElapsedMs` (`:139`) — the budget for the WHOLE loop, in-flight requests
    /// included, armed only when both knobs are positive (`:141-144`).
    max_elapsed_ms: i64,
    /// `AbortSignal.timeout(5000)` (`:150`) — the cap on each individual attempt.
    attempt_timeout: Duration,
}

impl CopilotRetryPolicy {
    /// What `loginGitHubCopilot` passes on both of its retry-loop paths: `{maxRetries: 2,
    /// maxElapsedMs: 5000}` to `fetchGitHubCopilotModels` (`:462-470`) and the same literal to
    /// `enableGitHubCopilotModel` → `fetchWithRateLimitRetry` (`:398`), each attempt capped at
    /// `:150`'s 5s.
    const LOGIN: Self = Self {
        max_retries: COPILOT_MAX_RATE_LIMIT_RETRIES,
        max_elapsed_ms: COPILOT_MAX_RATE_LIMIT_ELAPSED_MS,
        attempt_timeout: COPILOT_ATTEMPT_TIMEOUT,
    };
}

/// `expiresAt * 1000 - 5 * 60 * 1000` (`github-copilot.ts:274`).
const COPILOT_TOKEN_EXPIRY_SKEW_MS: i64 = 5 * 60 * 1000;

/// `OAuthCredential.enterpriseUrl` (`github-copilot.ts:275`), flattened into
/// [`Credential::Oauth`]'s `ext`. Mirrors the private constant of the same value in
/// `providers/github_copilot.rs`, which is the reader of this key.
const EXT_ENTERPRISE_URL: &str = "enterpriseUrl";
/// `OAuthCredential.availableModelIds` (`github-copilot.ts:286`, `:357`).
const EXT_AVAILABLE_MODEL_IDS: &str = "availableModelIds";

// ---------------------------------------------------------------------------
// getUrls — github-copilot.ts:51-61
// ---------------------------------------------------------------------------

/// The three domain-derived endpoints (`getUrls`, `github-copilot.ts:51-61`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubDeviceUrls {
    /// `https://{domain}/login/device/code` (`:57`).
    pub device_code_url: String,
    /// `https://{domain}/login/oauth/access_token` (`:58`).
    pub access_token_url: String,
    /// `https://api.{domain}/copilot_internal/v2/token` (`:59`).
    pub copilot_token_url: String,
}

// ---------------------------------------------------------------------------
// new URL(verificationUri) — github-copilot.ts:174-184
// ---------------------------------------------------------------------------

/// `new URL(value)` restricted to `http:`/`https:`, returning `.href` — `github-copilot.ts:174-184`
/// (and the identical guard at `kimi-coding.ts:57-67`). `None` is upstream's *either* failure: the
/// URL did not parse, or its protocol is neither http nor https.
///
/// The value is handed to the platform "open in browser" call, so a `file:`/`javascript:`/
/// `vbscript:` verification URI from a compromised or spoofed device endpoint must never survive
/// this function. That is the whole point of the guard upstream added.
///
/// Rust has no WHATWG URL parser here (this crate carries no `url` dependency; see
/// `Cargo.toml:22-33`), so `.href` normalization is implemented to the subset these endpoints can
/// produce: the scheme and host are ASCII-lowercased, and an empty path becomes `/`. Percent-
/// encoding normalization, IDNA/punycode and default-port elision are *not* applied — GitHub and
/// Kimi return already-normalized ASCII origins, and none of the three would change a URL that is
/// already in that form.
pub fn parse_http_href(value: &str) -> Option<String> {
    // `new URL("")` throws; so does anything with no scheme.
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
    // `parsedUri.protocol !== "https:" && parsedUri.protocol !== "http:"` (`:182`).
    if scheme != "http" && scheme != "https" {
        return None;
    }

    // http/https are WHATWG "special" schemes, so the parser's "special authority slashes" state
    // skips ANY run of `/` or `\` after the scheme — `new URL("https:/x")` and `new URL("https:x")`
    // both yield `https://x/`. What it will not tolerate is an empty host, which throws.
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
    // An IPv6 literal must be terminated, or `new URL` throws.
    if host_port.starts_with('[') && !host_port.contains(']') {
        return None;
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
        return None;
    }

    let authority_out = if userinfo.is_empty() {
        host_port.to_ascii_lowercase()
    } else {
        format!("{userinfo}@{}", host_port.to_ascii_lowercase())
    };
    // `new URL("https://host").href === "https://host/"`, and a bare `?`/`#` keeps its own slash.
    let tail_raw = if tail.is_empty() || tail.starts_with('?') || tail.starts_with('#') {
        format!("/{tail}")
    } else {
        tail.to_string()
    };
    let tail_out = percent_encode_c0(&tail_raw);
    Some(format!("{scheme}://{authority_out}{tail_out}"))
}

/// WHATWG "C0 control percent-encode set" — C0 controls (U+0000..U+001F) and every code point
/// above U+007E — applied to everything after the authority.
///
/// This is what makes `href` differ from the raw input, and it is load-bearing rather than
/// cosmetic: pi's own `packages/ai/test/github-copilot-oauth.test.ts` case "normalizes
/// verification_uri before it reaches onDeviceCode" feeds
/// `https://github.com/login/\u{1b}]8;;evil` and asserts `new URL(raw).href !== raw` and that the
/// RAW value never reaches `onDeviceCode`. `\u{1b}]8;;` is an OSC-8 hyperlink introducer: passed
/// through to a terminal it can retarget the link the user is told to open. Percent-encoding turns
/// it into the inert literal `%1B]8;;evil`.
fn percent_encode_c0(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if (ch as u32) <= 0x1F || (ch as u32) > 0x7E {
            let mut buf = [0u8; 4];
            for byte in ch.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(ch);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// DeviceCodeResponse — github-copilot.ts:20-26
// ---------------------------------------------------------------------------

/// The parsed `login/device/code` payload (`DeviceCodeResponse`, `github-copilot.ts:20-26`).
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    /// Already run through [`parse_http_href`] (`:189`).
    pub verification_uri: String,
    /// Optional upstream (`interval?: number`, `:24`); a present-but-non-numeric value is a
    /// [`INVALID_DEVICE_CODE_FIELDS`] failure, not a fallback.
    pub interval: Option<f64>,
    pub expires_in: f64,
}

/// `startDeviceFlow`'s field validation and URL guard (`github-copilot.ts:154-192`), split out so
/// it is assertable without a socket.
pub fn parse_device_code_response(raw: &Value) -> Result<DeviceCodeResponse, OAuthError> {
    // `:154-156` — `!data || typeof data !== "object"`. A JSON array is `typeof "object"` in JS but
    // then every field read is `undefined`, so it fails the field check either way.
    let Some(data) = raw.as_object() else {
        return Err(OAuthError::Failed(INVALID_DEVICE_CODE_RESPONSE.to_string()));
    };

    // `:158-172`. `interval` is the only optional field: absent is fine, present-and-not-a-number
    // (including JSON `null`, whose `typeof` is `"object"`) is not.
    let device_code = data.get("device_code").and_then(Value::as_str);
    let user_code = data.get("user_code").and_then(Value::as_str);
    let verification_uri = data.get("verification_uri").and_then(Value::as_str);
    let expires_in = data.get("expires_in").and_then(Value::as_f64);
    let interval = match data.get("interval") {
        None => None,
        Some(value) => match value.as_f64() {
            Some(n) => Some(n),
            None => return Err(OAuthError::Failed(INVALID_DEVICE_CODE_FIELDS.to_string())),
        },
    };
    let (Some(device_code), Some(user_code), Some(verification_uri), Some(expires_in)) =
        (device_code, user_code, verification_uri, expires_in)
    else {
        return Err(OAuthError::Failed(INVALID_DEVICE_CODE_FIELDS.to_string()));
    };

    // `:174-184`
    let Some(href) = parse_http_href(verification_uri) else {
        return Err(OAuthError::Failed(UNTRUSTED_VERIFICATION_URI.to_string()));
    };

    // `:186-192`
    Ok(DeviceCodeResponse {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        verification_uri: href,
        interval,
        expires_in,
    })
}

/// Map one `login/oauth/access_token` body onto a poll result (`github-copilot.ts:221-239`), split
/// out so every branch is assertable without a socket.
pub fn classify_device_token_response(raw: &Value) -> DeviceCodePollResult<String> {
    // `:221-223`
    if let Some(access_token) = raw
        .as_object()
        .and_then(|o| o.get("access_token"))
        .and_then(Value::as_str)
    {
        return DeviceCodePollResult::Complete(access_token.to_string());
    }

    // `:225-237`
    if let Some(error) = raw
        .as_object()
        .and_then(|o| o.get("error"))
        .and_then(Value::as_str)
    {
        if error == "authorization_pending" {
            return DeviceCodePollResult::Pending;
        }
        if error == "slow_down" {
            // `typeof interval === "number" ? interval : undefined` (`:232`) — note this branch
            // does NOT reject a non-numeric interval, unlike the device-code response.
            let interval_seconds = raw
                .as_object()
                .and_then(|o| o.get("interval"))
                .and_then(Value::as_f64);
            return DeviceCodePollResult::SlowDown { interval_seconds };
        }
        // `:235-236` — JS truthiness, so an empty `error_description` adds no suffix.
        let description = raw
            .as_object()
            .and_then(|o| o.get("error_description"))
            .and_then(Value::as_str)
            .filter(|d| !d.is_empty());
        let suffix = description.map(|d| format!(": {d}")).unwrap_or_default();
        return DeviceCodePollResult::Failed {
            message: format!("Device flow failed: {error}{suffix}"),
        };
    }

    // `:239`
    DeviceCodePollResult::Failed {
        message: INVALID_DEVICE_TOKEN_RESPONSE.to_string(),
    }
}

// ---------------------------------------------------------------------------
// The flow
// ---------------------------------------------------------------------------

/// Origins used by this module's loopback tests so nothing in the suite can reach a real GitHub
/// host. Production always leaves this `None` and derives all three from the domain and the
/// Copilot token's `proxy-ep=` claim. Mirrors the same seam in `providers/github_copilot.rs`.
#[derive(Clone, Debug)]
struct OriginOverride {
    /// Replaces `https://{domain}` for the two `login/…` endpoints.
    github_origin: String,
    /// Replaces `https://api.{domain}` for `copilot_internal/v2/token`.
    api_origin: String,
    /// Replaces `getGitHubCopilotBaseUrl(...)` for `/models` and `/models/{id}/policy`.
    copilot_origin: String,
}

/// GitHub Copilot's full OAuth strategy — upstream's `githubCopilotOAuth`
/// (`github-copilot.ts:367-379`).
///
/// `login` is ported here; `refresh` and `to_auth` delegate to
/// [`crate::providers::github_copilot::GitHubCopilotOAuth`], which already ports
/// `refreshGitHubCopilotToken` (`:282-288`) and `toAuth` (`:373-377`) verbatim.
pub struct GitHubCopilotLogin {
    /// Ambient context for `HTTP(S)_PROXY` / `NO_PROXY` resolution, so the login calls honour the
    /// same proxy policy as provider traffic (`stream/sse.rs:181-192`).
    auth_ctx: Arc<dyn AuthContext>,
    /// The runtime half of the same upstream object.
    runtime: GitHubCopilotOAuth,
    origins: Option<OriginOverride>,
    /// The retry policy every call through [`Self::fetch_with_rate_limit_retry`] runs under. Always
    /// [`CopilotRetryPolicy::LOGIN`] in production — the tests shorten it so a bound can be asserted
    /// in milliseconds instead of pi's five real seconds.
    retry_policy: CopilotRetryPolicy,
}

impl Default for GitHubCopilotLogin {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHubCopilotLogin {
    /// The production strategy.
    pub fn new() -> Self {
        Self::with_auth_context(Arc::new(EnvAuthContext))
    }

    /// Override the ambient auth context (for tests / custom env sources), mirroring
    /// [`crate::providers::github_copilot::GitHubCopilotOAuth::with_auth_context`]. The same
    /// context is threaded into the delegated runtime half so both use one proxy policy.
    pub fn with_auth_context(ctx: Arc<dyn AuthContext>) -> Self {
        Self {
            runtime: GitHubCopilotOAuth::new().with_auth_context(Arc::clone(&ctx)),
            auth_ctx: ctx,
            origins: None,
            retry_policy: CopilotRetryPolicy::LOGIN,
        }
    }

    /// Shorten the retry policy every retry-loop call runs under. Test-only seam (rule 4: a test
    /// must not spend pi's five real seconds proving a five-second cap); it does not exist in
    /// release builds.
    #[cfg(test)]
    fn with_retry_policy(mut self, policy: CopilotRetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    /// Point all three GitHub origins at a loopback listener. Test-only seam (rule 4: never hit
    /// the network in a test); it does not exist in release builds.
    #[cfg(test)]
    fn with_origins(mut self, github_origin: &str, api_origin: &str, copilot_origin: &str) -> Self {
        self.origins = Some(OriginOverride {
            github_origin: github_origin.to_string(),
            api_origin: api_origin.to_string(),
            copilot_origin: copilot_origin.to_string(),
        });
        self
    }

    /// `getUrls` (`github-copilot.ts:51-61`).
    pub fn urls(&self, domain: &str) -> GitHubDeviceUrls {
        let (github, api) = match &self.origins {
            Some(o) => (o.github_origin.clone(), o.api_origin.clone()),
            None => (format!("https://{domain}"), format!("https://api.{domain}")),
        };
        GitHubDeviceUrls {
            device_code_url: format!("{github}/login/device/code"),
            access_token_url: format!("{github}/login/oauth/access_token"),
            copilot_token_url: format!("{api}/copilot_internal/v2/token"),
        }
    }

    /// The `/models` and `/models/{id}/policy` origin (`github-copilot.ts:116-117`, `:295-296`).
    fn copilot_origin(&self, token: &str, enterprise_domain: Option<&str>) -> String {
        match &self.origins {
            Some(o) => o.copilot_origin.clone(),
            None => github_copilot_base_url(Some(token), enterprise_domain),
        }
    }

    /// The editor-identity header set (`COPILOT_HEADERS`, `github-copilot.ts:12-17`).
    fn copilot_headers() -> Vec<(&'static str, String)> {
        COPILOT_HEADERS
            .iter()
            .map(|(k, v)| (*k, (*v).to_string()))
            .collect()
    }

    /// `fetchJson` (`github-copilot.ts:126-137`): issue the request, raise
    /// `"{status} {statusText}: {text}"` on a non-2xx, else parse the body as JSON.
    ///
    /// `form` is the `URLSearchParams` body upstream passes on the two POSTs; `None` is a GET.
    async fn fetch_json(
        &self,
        url: &str,
        headers: &[(&str, String)],
        form: Option<String>,
        timeout: Option<Duration>,
    ) -> Result<Value, OAuthError> {
        let client =
            crate::stream::sse::build_client_for_target(url, self.auth_ctx.as_ref(), None, None)
                .await
                .map_err(|e| OAuthError::Failed(e.to_string()))?;
        let mut request = match &form {
            Some(_) => client.post(url),
            None => client.get(url),
        };
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        if let Some(body) = form {
            request = request.body(body);
        }

        let response = request
            .send()
            .await
            .map_err(|e| OAuthError::Failed(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            // `${response.status} ${response.statusText}: ${text}` (`:131`). reqwest exposes the
            // canonical reason phrase rather than the server's literal one; they agree for every
            // standard code.
            let text = response.text().await.unwrap_or_default();
            return Err(OAuthError::Failed(format!(
                "{} {}: {text}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            )));
        }
        response
            .json::<Value>()
            .await
            .map_err(|e| OAuthError::Failed(e.to_string()))
    }

    /// `startDeviceFlow` (`github-copilot.ts:139-193`).
    pub async fn start_device_flow(&self, domain: &str) -> Result<DeviceCodeResponse, OAuthError> {
        let urls = self.urls(domain);
        // `:143-151`
        let headers = [
            ("Accept", "application/json".to_string()),
            (
                "Content-Type",
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("User-Agent", DEVICE_FLOW_USER_AGENT.to_string()),
        ];
        let body = encode_query([("client_id", CLIENT_ID), ("scope", DEVICE_CODE_SCOPE)]);
        let raw = self
            .fetch_json(&urls.device_code_url, &headers, Some(body), None)
            .await?;
        parse_device_code_response(&raw)
    }

    /// `pollForGitHubAccessToken` (`github-copilot.ts:195-242`).
    pub async fn poll_for_github_access_token(
        &self,
        domain: &str,
        device: &DeviceCodeResponse,
        cancel: Option<CancelToken>,
    ) -> Result<String, OAuthError> {
        let options = DeviceCodePollOptions {
            // `:202-204`
            interval_seconds: device.interval,
            expires_in_seconds: Some(device.expires_in),
            wait_before_first_poll: true,
            cancel,
        };
        let poller = AccessTokenPoller {
            flow: self,
            access_token_url: self.urls(domain).access_token_url,
            device_code: device.device_code.clone(),
        };
        poll_oauth_device_code_flow(&options, &poller).await
    }

    /// `refreshGitHubCopilotAccessToken` (`github-copilot.ts:244-277`): trade the GitHub token for
    /// a short-lived Copilot token. See the module doc for why this is not the provider module's
    /// copy.
    async fn exchange_copilot_token(
        &self,
        github_token: &str,
        enterprise_domain: Option<&str>,
    ) -> Result<Credential, OAuthError> {
        // `:248` — `enterpriseDomain || "github.com"`, JS truthiness so `""` falls through.
        let domain = enterprise_domain
            .filter(|d| !d.is_empty())
            .unwrap_or(DEFAULT_GITHUB_DOMAIN);
        let urls = self.urls(domain);

        // `:251-257`
        let mut headers = vec![
            ("Accept", "application/json".to_string()),
            ("Authorization", format!("Bearer {github_token}")),
        ];
        headers.extend(Self::copilot_headers());
        let raw = self
            .fetch_json(&urls.copilot_token_url, &headers, None, None)
            .await?;

        // `:259-261`
        let Some(obj) = raw.as_object() else {
            return Err(OAuthError::Failed(
                INVALID_COPILOT_TOKEN_RESPONSE.to_string(),
            ));
        };
        // `:263-268`
        let token = obj.get("token").and_then(Value::as_str);
        let expires_at = obj.get("expires_at").and_then(Value::as_f64);
        let (Some(token), Some(expires_at)) = (token, expires_at) else {
            return Err(OAuthError::Failed(INVALID_COPILOT_TOKEN_FIELDS.to_string()));
        };

        // `:270-276`. JS spreads `enterpriseUrl: enterpriseDomain`, and an `undefined` value is
        // dropped by JSON serialization, so the key exists only when a domain was configured.
        let mut ext = Map::new();
        if let Some(domain) = enterprise_domain.filter(|d| !d.is_empty()) {
            ext.insert(
                EXT_ENTERPRISE_URL.to_string(),
                Value::String(domain.to_string()),
            );
        }
        Ok(Credential::Oauth {
            refresh: github_token.to_string(),
            access: token.to_string(),
            expires: (expires_at * 1000.0) as i64 - COPILOT_TOKEN_EXPIRY_SKEW_MS,
            ext,
        })
    }

    /// 1:1 port of pi `fetchWithRateLimitRetry` (`github-copilot.ts:135-166` @v0.87.1): issue the
    /// request, and retry **only** a 429, at most `COPILOT_MAX_RATE_LIMIT_RETRIES` times and only
    /// while the total wait stays inside `COPILOT_MAX_RATE_LIMIT_ELAPSED_MS`. A surviving 429 is
    /// RETURNED, not raised — each caller decides what it means.
    ///
    /// `retry-after` is read as float seconds first (`Number.parseFloat`, shared with the provider
    /// retry ladder), then as an HTTP-date; a value that is neither is pi's `!Number.isFinite`
    /// branch, which returns the response rather than guessing a delay. A negative delay clamps to
    /// zero, and a delay that would overrun the remaining budget returns the response too.
    ///
    /// Every attempt is bounded on BOTH axes pi bounds it on (`:148-151`): the caller's signal
    /// composed with the armed retry budget (`:145`) and with a fresh per-attempt
    /// `AbortSignal.timeout(5000)` (`:150`). So `max_elapsed_ms` caps total elapsed time including
    /// requests still in flight — not merely the sleeps between them — and a cancellation aborts a
    /// request mid-send rather than only at the next sleep.
    async fn fetch_with_rate_limit_retry(
        &self,
        url: &str,
        build: impl Fn(&reqwest::Client) -> reqwest::RequestBuilder,
        cancel: Option<&CancelToken>,
        policy: CopilotRetryPolicy,
    ) -> Result<reqwest::Response, OAuthError> {
        let client =
            crate::stream::sse::build_client_for_target(url, self.auth_ctx.as_ref(), None, None)
                .await
                .map_err(|e| OAuthError::Failed(e.to_string()))?;
        // pi `:141-146`: the budget exists only when BOTH knobs are positive.
        let deadline = (policy.max_retries > 0 && policy.max_elapsed_ms > 0)
            .then(|| now_millis() + policy.max_elapsed_ms);

        let mut retry: u32 = 0;
        loop {
            // pi `:148-151` — the request runs under `AbortSignal.any([requestSignal,
            // AbortSignal.timeout(5000)])`, so whichever of the per-attempt cap and the remaining
            // budget is shorter is what actually bounds this send. `reqwest`'s per-request timeout
            // covers the body read too, exactly as pi's signal stays attached to the `Response`.
            let mut effective = policy.attempt_timeout;
            if let Some(deadline) = deadline {
                let remaining = deadline - now_millis();
                if remaining <= 0 {
                    // The budget signal has already fired, so pi's `fetch` rejects before the
                    // request leaves: an abort, not a rate-limit response.
                    return Err(OAuthError::Failed(COPILOT_ATTEMPT_ABORTED.to_string()));
                }
                effective = effective.min(Duration::from_millis(remaining.unsigned_abs()));
            }
            let send = build(&client).timeout(effective).send();
            // pi `:145` — the caller's own signal is part of the composition, so a cancellation
            // aborts the in-flight request instead of waiting for the next sleep.
            let sent = match cancel {
                Some(cancel) => match cancel.run_until_cancelled(send).await {
                    Some(sent) => sent,
                    None => return Err(OAuthError::Cancelled),
                },
                None => send.await,
            };
            let response = sent.map_err(|e| {
                if e.is_timeout() {
                    OAuthError::Failed(COPILOT_ATTEMPT_ABORTED.to_string())
                } else {
                    OAuthError::Failed(e.to_string())
                }
            })?;
            if response.status().as_u16() != 429 || retry == policy.max_retries {
                return Ok(response);
            }

            // pi `:154-162`
            let mut delay_ms = 500i64.saturating_mul(1i64 << retry.min(31));
            if let Some(raw) = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .filter(|v| !v.is_empty())
            {
                match js_parse_float(raw) {
                    Some(seconds) => delay_ms = (seconds * 1000.0) as i64,
                    None => match parse_http_date_ms(raw) {
                        Some(at) => delay_ms = at - now_millis(),
                        // `Date.parse` of a non-date is NaN, so `Number.isFinite(delayMs)` is false
                        // and pi hands the 429 back untouched.
                        None => return Ok(response),
                    },
                }
            }
            delay_ms = delay_ms.max(0);
            if let Some(deadline) = deadline
                && delay_ms >= deadline - now_millis()
            {
                return Ok(response);
            }
            // pi `:163-164`: drop the body, then sleep abortably.
            drop(response);
            let sleep = tokio::time::sleep(Duration::from_millis(delay_ms.unsigned_abs()));
            match cancel {
                Some(cancel) => {
                    if cancel.run_until_cancelled(sleep).await.is_none() {
                        return Err(OAuthError::Cancelled);
                    }
                }
                None => sleep.await,
            }
            retry += 1;
        }
    }

    /// `fetchGitHubCopilotModels` (`github-copilot.ts:168-194` @v0.87.1).
    ///
    /// `allowPolicyFallback` is `baseUrl === "https://api.individual.githubcopilot.com"` (`:177`,
    /// under the comment at `:175-176`): some Individual accounts report
    /// `model_picker_enabled: false` for every row despite explicit enabled policies, and limiting
    /// the fallback to that endpoint keeps strict picker semantics everywhere else. It is derived
    /// from the REAL base url rather than [`Self::copilot_origin`], which the tests override to
    /// loopback — the override changes where the request goes, never which account type this is.
    ///
    /// The retry policy is pi's `{maxRetries: 2, maxElapsedMs: 5000}` (`:462-470`), which caps this
    /// GET at `:150`'s 5s per attempt and the whole loop at 5s of elapsed time.
    async fn fetch_model_catalog(
        &self,
        copilot_token: &str,
        enterprise_domain: Option<&str>,
        cancel: Option<&CancelToken>,
    ) -> Result<CopilotModelCatalog, OAuthError> {
        let base_url = self.copilot_origin(copilot_token, enterprise_domain);
        let allow_policy_fallback = github_copilot_base_url(Some(copilot_token), enterprise_domain)
            == GITHUB_COPILOT_BASE_URL;
        let url = format!("{base_url}/models");
        let token = copilot_token.to_string();

        let response = self
            .fetch_with_rate_limit_retry(
                &url,
                |client| {
                    let mut request = client
                        .get(&url)
                        .header("Accept", "application/json")
                        .header("Authorization", format!("Bearer {token}"))
                        .header("X-GitHub-Api-Version", COPILOT_API_VERSION);
                    for (name, value) in Self::copilot_headers() {
                        request = request.header(name, value);
                    }
                    request
                },
                cancel,
                self.retry_policy,
            )
            .await?;

        // `:190-192` — the same `"{status} {statusText}: {text}"` shape `fetchJson` raises.
        if !response.status().is_success() {
            return Err(OAuthError::Failed(status_error_message(response).await));
        }
        let raw: Value = response
            .json()
            .await
            .map_err(|e| OAuthError::Failed(e.to_string()))?;
        parse_github_copilot_model_catalog(&raw, allow_policy_fallback)
            .map_err(|e| OAuthError::Failed(e.to_string()))
    }

    /// `enableGitHubCopilotModel` (`github-copilot.ts:373-408` @v0.87.1): accept the model policy for
    /// one model id, through [`Self::fetch_with_rate_limit_retry`] under pi's `{maxRetries: 2,
    /// maxElapsedMs: 5000}` (`:398`).
    ///
    /// `Ok(response.ok)`, with transport and client-build failures still swallowed as `Ok(false)`
    /// (upstream's `catch { return false }`, `:400-403`). A per-attempt or retry-budget abort lands
    /// there too: pi's `if (signal.aborted) throw error` is false for it, because what aborted was
    /// the COMPOSED signal, not the caller's — so a policy POST that outlives its cap costs this one
    /// id and the batch continues. A 429 that SURVIVES the retry budget is the one failure upstream
    /// raises instead (`:404-406`) — it means the endpoint is rate-limiting the whole batch, so the
    /// caller stops rather than hammering it for every remaining id.
    async fn enable_model(
        &self,
        token: &str,
        model_id: &str,
        enterprise_domain: Option<&str>,
        cancel: Option<&CancelToken>,
    ) -> Result<bool, OAuthError> {
        let base_url = self.copilot_origin(token, enterprise_domain);
        // `:380` — upstream interpolates the id straight into the path; Copilot ids are
        // `[a-z0-9.-]` so there is nothing to escape.
        let url = format!("{base_url}/models/{model_id}/policy");
        let token = token.to_string();

        // `:382-399`
        let response = match self
            .fetch_with_rate_limit_retry(
                &url,
                |client| {
                    let mut request = client
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .header("Authorization", format!("Bearer {token}"));
                    for (name, value) in Self::copilot_headers() {
                        request = request.header(name, value);
                    }
                    request
                        .header("openai-intent", "chat-policy")
                        .header("x-interaction-type", "chat-policy")
                        .body(r#"{"state":"enabled"}"#)
                },
                cancel,
                self.retry_policy,
            )
            .await
        {
            Ok(response) => response,
            // `:401` — an abort propagates; anything else is `false`.
            Err(OAuthError::Cancelled) => return Err(OAuthError::Cancelled),
            Err(_) => return Ok(false),
        };
        if response.status().as_u16() == 429 {
            return Err(OAuthError::Failed(status_error_message(response).await));
        }
        Ok(response.status().is_success())
    }

    /// `enableGitHubCopilotModels` (`github-copilot.ts:414-432` @v0.87.1): a SEQUENTIAL pass over
    /// the requested ids, collecting the successes. Policy updates are best effort, so a failure is
    /// simply not collected — but an exhausted rate limit `break`s the whole batch, and a
    /// cancellation propagates.
    ///
    /// `[CYRUP-DELTA]` this replaces `enableAllGitHubCopilotModels`, which fanned out over the
    /// ENTIRE embedded catalog with `join_all` (pi v0.83.0's `Promise.all`) and discarded every
    /// result. Upstream now posts a policy only for the `"unconfigured"` ids the listing named, one
    /// at a time, and feeds the successes back into the credential.
    async fn enable_models(
        &self,
        token: &str,
        model_ids: &[String],
        enterprise_domain: Option<&str>,
        cancel: Option<&CancelToken>,
    ) -> Result<Vec<String>, OAuthError> {
        let mut enabled = Vec::new();
        for model_id in model_ids {
            match self
                .enable_model(token, model_id, enterprise_domain, cancel)
                .await
            {
                Ok(true) => enabled.push(model_id.clone()),
                Ok(false) => {}
                Err(OAuthError::Cancelled) => return Err(OAuthError::Cancelled),
                Err(_) => break,
            }
        }
        Ok(enabled)
    }

    /// `loginGitHubCopilot` (`github-copilot.ts:434-485` @v0.87.1, wired as `githubCopilotOAuth.login`
    /// at `:496`).
    async fn run_login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        // `:330-334`
        let input = interaction
            .prompt(
                AuthPrompt::text(ENTERPRISE_PROMPT_MESSAGE)
                    .with_placeholder(ENTERPRISE_PROMPT_PLACEHOLDER),
            )
            .await?;
        // `:335`
        if interaction.cancel().is_some_and(CancelToken::is_cancelled) {
            return Err(OAuthError::Cancelled);
        }

        // `:337-340`
        let trimmed = input.trim();
        let enterprise_domain = normalize_domain(&input);
        if !trimmed.is_empty() && enterprise_domain.is_none() {
            return Err(OAuthError::Failed(INVALID_ENTERPRISE_DOMAIN.to_string()));
        }
        let domain = enterprise_domain
            .clone()
            .unwrap_or_else(|| DEFAULT_GITHUB_DOMAIN.to_string());

        // `:342-349`
        let device = self.start_device_flow(&domain).await?;
        interaction.notify(AuthEvent::DeviceCode {
            user_code: device.user_code.clone(),
            verification_uri: device.verification_uri.clone(),
            interval_seconds: device.interval,
            expires_in_seconds: Some(device.expires_in),
        });

        // `:351`
        let github_access_token = self
            .poll_for_github_access_token(&domain, &device, interaction.cancel().cloned())
            .await?;

        // `:352`
        let credential = self
            .exchange_copilot_token(&github_access_token, enterprise_domain.as_deref())
            .await?;
        let (refresh, access, expires, mut ext) = match credential {
            Credential::Oauth {
                refresh,
                access,
                expires,
                ext,
            } => (refresh, access, expires, ext),
            // Unreachable: `exchange_copilot_token` only ever builds `Credential::Oauth`.
            Credential::ApiKey { .. } => {
                return Err(OAuthError::Failed(
                    INVALID_COPILOT_TOKEN_RESPONSE.to_string(),
                ));
            }
        };

        // `:465-483` @v0.87.1. The LISTING RUNS FIRST now, and the policy pass runs only for the
        // ids it reported as `"unconfigured"`. The old comment here ("the policy pass runs BEFORE
        // the listing, so a model enabled here shows up in availableModelIds") described v0.83.0 and
        // is superseded: upstream unions the newly-enabled ids into `availableModelIds` instead of
        // relying on the listing to observe them. This is NOT a CYRUP-DELTA — it is the current
        // upstream order (PROV-098).
        let catalog = self
            .fetch_model_catalog(&access, enterprise_domain.as_deref(), interaction.cancel())
            .await?;

        // `:472-481` — `if (models.policyModelIds.length > 0)`: no ids to configure means no
        // progress notification and no requests at all.
        let mut enabled_model_ids: Vec<String> = Vec::new();
        if !catalog.policy_model_ids.is_empty() {
            interaction.notify(AuthEvent::Progress {
                message: ENABLING_MODELS_MESSAGE.to_string(),
            });
            enabled_model_ids = self
                .enable_models(
                    &access,
                    &catalog.policy_model_ids,
                    enterprise_domain.as_deref(),
                    interaction.cancel(),
                )
                .await?;
        }

        // `:484` — `[...new Set([...availableModelIds, ...enabledModelIds])]`. A JS `Set` preserves
        // INSERTION order and this lands in a JSON array, so the dedupe keeps first-seen order
        // rather than sorting.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let ids: Vec<Value> = catalog
            .available_model_ids
            .iter()
            .chain(enabled_model_ids.iter())
            .filter(|id| seen.insert(id.as_str()))
            .map(|id| Value::String(id.clone()))
            .collect();
        ext.insert(EXT_AVAILABLE_MODEL_IDS.to_string(), Value::Array(ids));
        Ok(Credential::Oauth {
            refresh,
            access,
            expires,
            ext,
        })
    }
}

/// `` `${response.status} ${response.statusText}: ${await response.text()}` `` — the message shape
/// upstream raises from `fetchJson` (`github-copilot.ts:199`), `fetchGitHubCopilotModels` (`:191`)
/// and `enableGitHubCopilotModel` (`:405`). reqwest exposes the canonical reason phrase rather than
/// the server's literal one; they agree for every standard code.
async fn status_error_message(response: reqwest::Response) -> String {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    format!(
        "{} {}: {text}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    )
}

/// The `poll` callback of `pollForGitHubAccessToken` (`github-copilot.ts:206-240`).
struct AccessTokenPoller<'a> {
    flow: &'a GitHubCopilotLogin,
    access_token_url: String,
    device_code: String,
}

#[async_trait::async_trait]
impl DeviceCodePoller for AccessTokenPoller<'_> {
    type Value = String;

    async fn poll(&self) -> Result<DeviceCodePollResult<String>, OAuthError> {
        // `:207-219`
        let headers = [
            ("Accept", "application/json".to_string()),
            (
                "Content-Type",
                "application/x-www-form-urlencoded".to_string(),
            ),
            ("User-Agent", DEVICE_FLOW_USER_AGENT.to_string()),
        ];
        let body = encode_query([
            ("client_id", CLIENT_ID),
            ("device_code", self.device_code.as_str()),
            ("grant_type", DEVICE_CODE_GRANT_TYPE),
        ]);
        // `fetchJson` rejects on a non-2xx, and that rejection propagates out of the poll loop
        // rather than being folded into a poll status — matching upstream, where the `await`
        // at `:207` is unguarded.
        let raw = self
            .flow
            .fetch_json(&self.access_token_url, &headers, Some(body), None)
            .await?;
        Ok(classify_device_token_response(&raw))
    }
}

#[async_trait::async_trait]
impl OAuthAuth for GitHubCopilotLogin {
    /// `github-copilot.ts:368`.
    fn name(&self) -> &str {
        "GitHub Copilot"
    }

    /// `isSubscription: true` (pi v0.84.1 `oauth/github-copilot.ts:402`) — a Copilot seat.
    fn is_subscription(&self) -> bool {
        true
    }

    /// 1:1 port of `loginGitHubCopilot` (`github-copilot.ts:434-485` @v0.87.1, wired at `:496`): prompt for
    /// an optional GitHub Enterprise domain, run the RFC 8628 device grant against that domain,
    /// exchange the resulting GitHub token for a Copilot token, accept every model policy, then
    /// record the account's selectable model ids on the credential.
    async fn login(
        &self,
        interaction: &dyn AuthInteraction,
        _options: &LoginOptions,
    ) -> Result<Credential, OAuthError> {
        self.run_login(interaction).await
    }

    /// `github-copilot.ts:370` — delegated to the provider module's port of
    /// `refreshGitHubCopilotToken` (`:282-288`).
    async fn refresh(&self, cred: &Credential) -> Result<Credential, AuthError> {
        self.runtime.refresh(cred).await
    }

    /// `github-copilot.ts:373-377` — delegated to the provider module's port of `toAuth`.
    async fn to_auth(&self, cred: &Credential) -> Result<ModelAuth, AuthError> {
        self.runtime.to_auth(cred).await
    }
}

/// The provider id this flow's failures are attributed to.
pub fn provider_id() -> &'static str {
    GITHUB_COPILOT_PROVIDER_ID
}

#[cfg(test)]
mod tests {
    //! (the upstream normalization case is `normalizes_verification_uri_before_it_reaches_the_user`
    //! below — pi ships it at `packages/ai/test/github-copilot-oauth.test.ts`)

    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::auth::oauth::interaction::ScriptedInteraction;
    use base64::Engine as _;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // ------------------------------------------------------------------ upstream fixtures

    /// The device-code payload GitHub returns (`github-copilot.ts:20-26`).
    fn device_code_body() -> Value {
        json!({
            "device_code": "3584d83530557fdd1f46af8289938c8ef79f9dc5",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 899.0,
            "interval": 0.0,
        })
    }

    /// A `copilot_internal/v2/token` payload, including the `proxy-ep=` claim `toAuth` reads.
    fn copilot_token_body() -> Value {
        json!({
            "token": "tid=abc;exp=1800000000;proxy-ep=proxy.individual.githubcopilot.com;",
            "expires_at": 1_800_000_000.0,
        })
    }

    /// A `/models` payload exercising all three `isSelectableCopilotModel` rules.
    fn models_body() -> Value {
        json!({"data": [
            {"id": "gpt-5.5", "model_picker_enabled": true},
            {"id": "claude-sonnet-4.6", "model_picker_enabled": true,
             "capabilities": {"supports": {"tool_calls": true}}},
            {"id": "disabled-by-policy", "model_picker_enabled": true,
             "policy": {"state": "disabled"}},
            {"id": "not-in-picker", "model_picker_enabled": false},
            {"id": "no-tools", "model_picker_enabled": true,
             "capabilities": {"supports": {"tool_calls": false}}},
        ]})
    }

    // ------------------------------------------------------------------ loopback harness

    /// One recorded request: `"{METHOD} {PATH}"` plus the raw body.
    #[derive(Clone, Debug)]
    struct Recorded {
        line: String,
        head: String,
        body: String,
    }

    type Router = Arc<dyn Fn(&str, usize) -> (u16, String) + Send + Sync>;

    /// A loopback HTTP server. `router(path, nth_hit_for_that_path)` picks the reply, so a poll
    /// sequence (pending, then success) is scriptable. Nothing in this file can reach a real host.
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
                    // Read the head, then whatever Content-Length announces.
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
                    sink.lock().unwrap().push(Recorded {
                        line: line.clone(),
                        head: head.clone(),
                        body,
                    });

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

    /// A [`spawn`] whose router also picks response HEADERS, so a `retry-after` can be scripted.
    /// PROV-098: [`spawn`]'s `(status, body)` router cannot express one, and the 429 retry path is
    /// defined by that header.
    type HeaderRouter =
        Arc<dyn Fn(&str, usize) -> (u16, Vec<(String, String)>, String) + Send + Sync>;

    async fn spawn_with_headers(router: HeaderRouter) -> (String, Arc<Mutex<Vec<Recorded>>>) {
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
                    sink.lock().unwrap().push(Recorded {
                        line: line.clone(),
                        head: head.clone(),
                        body,
                    });

                    let (status, extra, reply) = router(&path, nth);
                    // PROV-098 sentinel: status 0 means STALL — accept the request, hold the socket
                    // open and never answer. It is how a test scripts a server that is reachable but
                    // unresponsive, which is the only shape that can observe a per-attempt or
                    // retry-budget abort (`github-copilot.ts:148-151` @v0.87.1).
                    if status == 0 {
                        tokio::time::sleep(Duration::from_secs(600)).await;
                        return;
                    }
                    let mut response =
                        format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n");
                    for (name, value) in extra {
                        response.push_str(&format!("{name}: {value}\r\n"));
                    }
                    response.push_str(&format!(
                        "Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    ));
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        (format!("http://{addr}"), log)
    }

    /// An [`AuthContext`] over a fixed map — keeps proxy resolution deterministic so an ambient
    /// `HTTPS_PROXY` cannot reroute a loopback request.
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
    use crate::providers::github_copilot::github_copilot_models;

    fn empty_env() -> Arc<dyn AuthContext> {
        Arc::new(MapEnv(BTreeMap::new()))
    }

    fn flow(origin: &str) -> GitHubCopilotLogin {
        GitHubCopilotLogin::with_auth_context(empty_env()).with_origins(origin, origin, origin)
    }

    // ------------------------------------------------------------------ constants

    /// The client id is upstream's base64 literal, decoded (`github-copilot.ts:9-10`).
    #[test]
    fn client_id_matches_upstream_base64() {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(CLIENT_ID_BASE64)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), CLIENT_ID);
        assert_eq!(CLIENT_ID, "Iv1.b507a08c87ecfe98");
    }

    /// `getUrls` (`github-copilot.ts:56-60`), for both the default and an enterprise domain. The
    /// Copilot token host gains an `api.` prefix while the two `login/…` hosts do not.
    #[test]
    fn urls_match_upstream_for_default_and_enterprise_domains() {
        let flow = GitHubCopilotLogin::with_auth_context(empty_env());
        assert_eq!(
            flow.urls("github.com"),
            GitHubDeviceUrls {
                device_code_url: "https://github.com/login/device/code".into(),
                access_token_url: "https://github.com/login/oauth/access_token".into(),
                copilot_token_url: "https://api.github.com/copilot_internal/v2/token".into(),
            }
        );
        assert_eq!(
            flow.urls("company.ghe.com"),
            GitHubDeviceUrls {
                device_code_url: "https://company.ghe.com/login/device/code".into(),
                access_token_url: "https://company.ghe.com/login/oauth/access_token".into(),
                copilot_token_url: "https://api.company.ghe.com/copilot_internal/v2/token".into(),
            }
        );
    }

    // ------------------------------------------------------------------ parse_http_href

    /// `github-copilot.ts:174-184`: the verification URI is opened in a browser, so anything that
    /// is not http(s) is refused. The MIRROR case (a real GitHub verification URI) stays green, so
    /// the rejections are not vacuous.
    #[test]
    fn verification_uri_must_parse_and_be_http() {
        // MIRROR — the URI GitHub actually returns survives untouched.
        assert_eq!(
            parse_http_href("https://github.com/login/device").as_deref(),
            Some("https://github.com/login/device")
        );
        assert_eq!(
            parse_http_href("http://127.0.0.1:8080/login/device?x=1").as_deref(),
            Some("http://127.0.0.1:8080/login/device?x=1")
        );

        // The attacks the upstream guard exists for.
        assert_eq!(parse_http_href("file:///etc/passwd"), None);
        assert_eq!(parse_http_href("javascript:alert(1)"), None);
        assert_eq!(parse_http_href("vbscript:msgbox(1)"), None);
        assert_eq!(parse_http_href("/usr/bin/open"), None);
        assert_eq!(parse_http_href(""), None);
        // `new URL("https://")` throws: a special scheme needs a host. So does an unterminated
        // IPv6 literal.
        assert_eq!(parse_http_href("https://"), None);
        assert_eq!(parse_http_href("https://[::1"), None);
    }

    /// WHATWG's "special authority slashes" state skips any run of `/` or `\` after a special
    /// scheme, so `new URL("https:/x")` and `new URL("https:x")` both parse to `https://x/`.
    /// Refusing them here would reject a verification URI upstream accepts.
    #[test]
    fn special_scheme_tolerates_missing_slashes_like_whatwg() {
        assert_eq!(
            parse_http_href("https:/github.com").as_deref(),
            Some("https://github.com/")
        );
        assert_eq!(
            parse_http_href("https:github.com/login/device").as_deref(),
            Some("https://github.com/login/device")
        );
    }

    /// `.href` normalization (`:189`): the scheme and host lowercase, and an empty path becomes
    /// `/`, exactly as `new URL(...).href` renders them.
    #[test]
    fn verification_uri_is_normalized_like_url_href() {
        assert_eq!(
            parse_http_href("HTTPS://GitHub.COM").as_deref(),
            Some("https://github.com/")
        );
        assert_eq!(
            parse_http_href("https://GitHub.com/Login/Device").as_deref(),
            Some("https://github.com/Login/Device"),
            "the path keeps its case; only the host is lowercased"
        );
        assert_eq!(
            parse_http_href("https://github.com?a=b").as_deref(),
            Some("https://github.com/?a=b")
        );
    }

    // ------------------------------------------------------------------ device-code parsing

    /// The happy path (`github-copilot.ts:186-192`).
    #[test]
    fn device_code_response_parses_upstream_shape() {
        let device = parse_device_code_response(&device_code_body()).unwrap();
        assert_eq!(device.user_code, "WDJB-MJHT");
        assert_eq!(device.verification_uri, "https://github.com/login/device");
        assert_eq!(device.expires_in, 899.0);
        assert_eq!(device.interval, Some(0.0));
    }

    /// `:154-156` and `:164-172`: the four field guards, each with its upstream message.
    #[test]
    fn device_code_response_field_guards() {
        let err = |v: Value| parse_device_code_response(&v).unwrap_err().to_string();

        assert_eq!(err(json!("nope")), "Invalid device code response");
        assert_eq!(err(json!(null)), "Invalid device code response");

        let mut missing_code = device_code_body();
        missing_code.as_object_mut().unwrap().remove("device_code");
        assert_eq!(err(missing_code), "Invalid device code response fields");

        let mut numeric_user_code = device_code_body();
        numeric_user_code.as_object_mut().unwrap()["user_code"] = json!(42);
        assert_eq!(
            err(numeric_user_code),
            "Invalid device code response fields"
        );

        let mut string_expires = device_code_body();
        string_expires.as_object_mut().unwrap()["expires_in"] = json!("899");
        assert_eq!(err(string_expires), "Invalid device code response fields");

        // `interval !== undefined && typeof interval !== "number"` (`:168`): absent is fine,
        // JSON `null` is not — its `typeof` is `"object"`.
        let mut no_interval = device_code_body();
        no_interval.as_object_mut().unwrap().remove("interval");
        assert_eq!(
            parse_device_code_response(&no_interval).unwrap().interval,
            None
        );
        let mut null_interval = device_code_body();
        null_interval.as_object_mut().unwrap()["interval"] = json!(null);
        assert_eq!(err(null_interval), "Invalid device code response fields");

        // The URL guard (`:180`/`:183`) reports its own message, not the field one.
        let mut hostile = device_code_body();
        hostile.as_object_mut().unwrap()["verification_uri"] = json!("file:///etc/passwd");
        assert_eq!(
            err(hostile),
            "Untrusted verification_uri in device code response"
        );
    }

    // ------------------------------------------------------------------ poll classification

    /// Every branch of `github-copilot.ts:221-239`, against the literal payloads GitHub's device
    /// endpoint returns.
    #[test]
    fn device_token_response_classification() {
        assert_eq!(
            classify_device_token_response(
                &json!({"access_token": "gho_x", "token_type": "bearer", "scope": "read:user"})
            ),
            DeviceCodePollResult::Complete("gho_x".to_string())
        );
        assert_eq!(
            classify_device_token_response(&json!({"error": "authorization_pending",
                "error_description": "The authorization request is still pending."})),
            DeviceCodePollResult::Pending
        );
        assert_eq!(
            classify_device_token_response(&json!({"error": "slow_down", "interval": 10})),
            DeviceCodePollResult::SlowDown {
                interval_seconds: Some(10.0)
            }
        );
        // A non-numeric interval on `slow_down` is dropped, not fatal (`:232`).
        assert_eq!(
            classify_device_token_response(&json!({"error": "slow_down", "interval": "10"})),
            DeviceCodePollResult::SlowDown {
                interval_seconds: None
            }
        );
        assert_eq!(
            classify_device_token_response(&json!({"error": "access_denied",
                "error_description": "The user has denied your application access."})),
            DeviceCodePollResult::Failed {
                message: "Device flow failed: access_denied: The user has denied your application access.".to_string()
            }
        );
        // JS truthiness: an empty description contributes no suffix (`:235`).
        assert_eq!(
            classify_device_token_response(
                &json!({"error": "expired_token", "error_description": ""})
            ),
            DeviceCodePollResult::Failed {
                message: "Device flow failed: expired_token".to_string()
            }
        );
        assert_eq!(
            classify_device_token_response(&json!({"unexpected": true})),
            DeviceCodePollResult::Failed {
                message: "Invalid device token response".to_string()
            }
        );
    }

    // ------------------------------------------------------------------ request shapes

    /// `startDeviceFlow` posts exactly `client_id` + `scope=read:user`, form-encoded, with the
    /// three upstream headers (`github-copilot.ts:141-152`).
    #[tokio::test]
    async fn start_device_flow_sends_the_upstream_request() {
        let body = device_code_body().to_string();
        let (origin, log) = spawn(Arc::new(move |_, _| (200, body.clone()))).await;
        let device = flow(&origin).start_device_flow("github.com").await.unwrap();
        assert_eq!(
            device.device_code,
            "3584d83530557fdd1f46af8289938c8ef79f9dc5"
        );

        let recorded = log.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1);
        let request = &recorded[0];
        assert_eq!(request.line.split(' ').next().unwrap(), "POST");
        assert_eq!(
            request.line.split(' ').nth(1).unwrap(),
            "/login/device/code"
        );
        assert_eq!(
            request.body,
            "client_id=Iv1.b507a08c87ecfe98&scope=read%3Auser"
        );
        let head = request.head.to_ascii_lowercase();
        assert!(head.contains("accept: application/json"), "{head}");
        assert!(
            head.contains("content-type: application/x-www-form-urlencoded"),
            "{head}"
        );
        assert!(
            head.contains("user-agent: githubcopilotchat/0.35.0"),
            "{head}"
        );
    }

    /// `fetchJson`'s non-2xx message (`github-copilot.ts:128-132`) propagates verbatim.
    #[tokio::test]
    async fn non_2xx_device_code_response_raises_status_and_body() {
        let (origin, _) = spawn(Arc::new(|_, _| (404, "not found".to_string()))).await;
        let err = flow(&origin)
            .start_device_flow("github.com")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "404 Not Found: not found");
    }

    /// The poll body is the RFC 8628 device-code grant (`github-copilot.ts:214-218`).
    #[tokio::test]
    async fn access_token_poll_sends_the_device_code_grant() {
        let reply = json!({"access_token": "gho_live"}).to_string();
        let (origin, log) = spawn(Arc::new(move |_, _| (200, reply.clone()))).await;
        let flow = flow(&origin);
        let poller = AccessTokenPoller {
            flow: &flow,
            access_token_url: flow.urls("github.com").access_token_url,
            device_code: "dev-code-1".to_string(),
        };
        assert_eq!(
            poller.poll().await.unwrap(),
            DeviceCodePollResult::Complete("gho_live".to_string())
        );
        let recorded = log.lock().unwrap().clone();
        assert_eq!(
            recorded[0].body,
            "client_id=Iv1.b507a08c87ecfe98&device_code=dev-code-1&grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code"
        );
        assert_eq!(
            recorded[0].line.split(' ').nth(1).unwrap(),
            "/login/oauth/access_token"
        );
    }

    /// A transport/status failure inside `poll` propagates out of the loop rather than reading as
    /// `pending` — upstream's `await fetchJson(...)` at `:207` is unguarded.
    #[tokio::test]
    async fn poll_propagates_a_failed_token_request() {
        let (origin, _) = spawn(Arc::new(|_, _| (503, "upstream down".to_string()))).await;
        let flow = flow(&origin);
        let poller = AccessTokenPoller {
            flow: &flow,
            access_token_url: flow.urls("github.com").access_token_url,
            device_code: "dev-code-1".to_string(),
        };
        assert_eq!(
            poller.poll().await.unwrap_err().to_string(),
            "503 Service Unavailable: upstream down"
        );
    }

    /// `enableGitHubCopilotModel` (`github-copilot.ts:373-408` @v0.87.1): `Ok(response.ok)`, and a
    /// non-429 failure is still swallowed as `Ok(false)` so one refused policy cannot fail a login.
    #[tokio::test]
    async fn enable_model_reports_ok_and_swallows_failures() {
        let (origin, log) = spawn(Arc::new(|_, nth| {
            if nth == 0 {
                (200, "{}".to_string())
            } else {
                (403, "forbidden".to_string())
            }
        }))
        .await;
        let flow = flow(&origin);
        assert!(
            flow.enable_model("copilot-token", "claude-sonnet-4.6", None, None)
                .await
                .expect("a 200 is not an error")
        );
        assert!(
            !flow
                .enable_model("copilot-token", "claude-sonnet-4.6", None, None)
                .await
                .expect("a 403 is swallowed, not raised")
        );

        let recorded = log.lock().unwrap().clone();
        assert_eq!(
            recorded[0].line.split(' ').nth(1).unwrap(),
            "/models/claude-sonnet-4.6/policy"
        );
        assert_eq!(recorded[0].body, r#"{"state":"enabled"}"#);
        let head = recorded[0].head.to_ascii_lowercase();
        assert!(head.contains("openai-intent: chat-policy"), "{head}");
        assert!(head.contains("x-interaction-type: chat-policy"), "{head}");
        assert!(
            head.contains("copilot-integration-id: vscode-chat"),
            "{head}"
        );
        assert!(
            head.contains("authorization: bearer copilot-token"),
            "{head}"
        );
    }

    /// A dead policy endpoint is still just `Ok(false)` — no panic, no error (`:400-403`).
    #[tokio::test]
    async fn enable_model_swallows_a_dead_endpoint() {
        // Port 1 on loopback refuses instantly and is never a real service.
        let flow = GitHubCopilotLogin::with_auth_context(empty_env()).with_origins(
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
        );
        assert!(
            !flow
                .enable_model("t", "gpt-5.5", None, None)
                .await
                .expect("a dead endpoint is swallowed")
        );
    }

    // ------------------------------------------------------------------ login

    /// The full `loginGitHubCopilot` sequence (`github-copilot.ts:434-485` @v0.87.1) over loopback:
    /// prompt → device code → poll → Copilot token → model listing → policy pass → credential.
    ///
    /// PROV-098 rewrote the tail of this test rather than replacing it. Upstream now fetches the
    /// listing FIRST and posts a policy only for the ids it reported as `"unconfigured"`; no row in
    /// [`models_body`] carries a policy state, so this fixture produces NO policy call and NO
    /// "Enabling models..." notification at all. The per-`unconfigured`-id behaviour has its own
    /// tests below.
    #[tokio::test]
    async fn login_completes_the_device_flow_and_records_available_models() {
        let device = device_code_body().to_string();
        let token = copilot_token_body().to_string();
        let models = models_body().to_string();
        let (origin, log) = spawn(Arc::new(move |path, nth| match path {
            "/login/device/code" => (200, device.clone()),
            // First poll pending, second returns the token (`:227-228` then `:221-222`).
            "/login/oauth/access_token" if nth == 0 => {
                (200, json!({"error": "authorization_pending"}).to_string())
            }
            "/login/oauth/access_token" => (200, json!({"access_token": "gho_user"}).to_string()),
            "/copilot_internal/v2/token" => (200, token.clone()),
            "/models" => (200, models.clone()),
            _ => (200, "{}".to_string()),
        }))
        .await;

        let interaction = ScriptedInteraction::new(vec![Ok(String::new())]);
        let credential = flow(&origin)
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap();

        // The credential (`:270-276` + `:355-358`).
        let (refresh, access, expires, ext) = match credential {
            Credential::Oauth {
                refresh,
                access,
                expires,
                ext,
            } => (refresh, access, expires, ext),
            Credential::ApiKey { .. } => panic!("login must yield an OAuth credential"),
        };
        assert_eq!(refresh, "gho_user", "the GitHub token is the refresh token");
        assert!(access.contains("proxy-ep=proxy.individual.githubcopilot.com"));
        // `expires_at * 1000 - 5 * 60 * 1000` (`:274`).
        assert_eq!(expires, 1_800_000_000_000i64 - 300_000);
        // Blank enterprise input: no `enterpriseUrl` key at all (`:275`, undefined dropped).
        assert!(!ext.contains_key("enterpriseUrl"));
        assert_eq!(
            ext.get("availableModelIds").unwrap(),
            &json!(["gpt-5.5", "claude-sonnet-4.6"]),
            "only picker-enabled, non-disabled, tool-calling models"
        );

        // The prompt (`:330-334`).
        let prompts = interaction.prompts();
        assert_eq!(prompts.len(), 1);
        assert_eq!(
            prompts[0].message,
            "GitHub Enterprise URL/domain (blank for github.com)"
        );
        assert_eq!(prompts[0].placeholder.as_deref(), Some("company.ghe.com"));

        // The events (`:446-452`; no `progress`, because `policyModelIds` is empty at `:472`).
        let events = interaction.events();
        assert_eq!(
            events[0],
            AuthEvent::DeviceCode {
                user_code: "WDJB-MJHT".to_string(),
                verification_uri: "https://github.com/login/device".to_string(),
                interval_seconds: Some(0.0),
                expires_in_seconds: Some(899.0),
            }
        );
        assert_eq!(
            events.len(),
            1,
            "no progress event without policy ids: {events:?}"
        );

        // No policy call at all, and the listing is still fetched exactly once.
        let recorded = log.lock().unwrap().clone();
        let paths: Vec<String> = recorded
            .iter()
            .map(|r| r.line.split(' ').nth(1).unwrap_or("").to_string())
            .collect();
        assert_eq!(paths.iter().filter(|p| *p == "/models").count(), 1);
        assert!(
            !paths.iter().any(|p| p.ends_with("/policy")),
            "no row is `unconfigured`, so no policy is posted: {paths:?}"
        );
        assert!(
            github_copilot_models().len() > 1,
            "the embedded catalog is non-empty, so the OLD fan-out would have shown up above"
        );
    }

    // ------------------------------------------------- PROV-098: policy pass + rate-limit retry

    /// PROV-098. Translated from `packages/ai/test/github-copilot-oauth.test.ts` @v0.87.1.
    mod prov098 {
        use super::*;

        /// A `/models` body covering every `policyModelIds` predicate at once. `gpt-5.5` and
        /// `claude-sonnet-4.6` are real embedded-catalog ids; `not-in-our-catalog` is not.
        fn policy_models_body() -> Value {
            json!({"data": [
                // `unconfigured` + picker-enabled + in the catalog → the ONLY policy target.
                {"id": "gpt-5.5", "model_picker_enabled": true,
                 "policy": {"state": "unconfigured"}},
                // Already `enabled` → available, never posted.
                {"id": "claude-sonnet-4.6", "model_picker_enabled": true,
                 "policy": {"state": "enabled"}},
                // `unconfigured` but NOT in the embedded catalog → skipped
                // (`Object.hasOwn(GITHUB_COPILOT_MODELS, id)`).
                {"id": "not-in-our-catalog", "model_picker_enabled": true,
                 "policy": {"state": "unconfigured"}},
                // `unconfigured` but not picker-enabled, and the picker set is non-empty so there is
                // no fallback → skipped.
                {"id": "claude-opus-4.1", "model_picker_enabled": false,
                 "policy": {"state": "unconfigured"}},
                // `disabled` → neither available nor posted.
                {"id": "disabled-by-policy", "model_picker_enabled": true,
                 "policy": {"state": "disabled"}},
                // No policy state at all → available, never posted.
                {"id": "no-policy-state", "model_picker_enabled": true},
            ]})
        }

        /// Everything except `/models` and `/models/*/policy`, so each test only scripts its own
        /// endpoint. `enterprise` picks the Copilot token, which is what decides
        /// `allow_policy_fallback` (the loopback origin override never does).
        fn base_reply(path: &str, nth: usize, enterprise: bool) -> Option<(u16, String)> {
            let token = if enterprise {
                // No `proxy-ep=`, so the base url falls through to `copilot-api.<domain>`.
                json!({"token": "tid=abc;exp=1800000000;", "expires_at": 1_800_000_000.0})
            } else {
                copilot_token_body()
            };
            match path {
                "/login/device/code" => Some((200, device_code_body().to_string())),
                "/login/oauth/access_token" if nth == 0 => {
                    Some((200, json!({"error": "authorization_pending"}).to_string()))
                }
                "/login/oauth/access_token" => {
                    Some((200, json!({"access_token": "gho_user"}).to_string()))
                }
                "/copilot_internal/v2/token" => Some((200, token.to_string())),
                _ => None,
            }
        }

        fn available_ids(credential: &Credential) -> Vec<String> {
            let Credential::Oauth { ext, .. } = credential else {
                panic!("login must yield an OAuth credential");
            };
            ext.get("availableModelIds")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        }

        fn policy_paths(log: &Arc<Mutex<Vec<Recorded>>>) -> Vec<String> {
            log.lock()
                .unwrap()
                .iter()
                .filter_map(|r| r.line.split(' ').nth(1).map(str::to_string))
                .filter(|p| p.ends_with("/policy"))
                .collect()
        }

        /// (a) `:127-131` — exactly one policy POST, for the single in-catalog, picker-enabled,
        /// `unconfigured` id. Never for the `enabled` one, never for a model cyrup cannot serve.
        #[tokio::test]
        async fn only_unconfigured_in_catalog_picker_models_get_a_policy() {
            let models = policy_models_body().to_string();
            let (origin, log) = spawn_with_headers(Arc::new(move |path, nth| {
                if let Some((status, body)) = base_reply(path, nth, false) {
                    return (status, Vec::new(), body);
                }
                match path {
                    "/models" => (200, Vec::new(), models.clone()),
                    _ => (200, Vec::new(), "{}".to_string()),
                }
            }))
            .await;

            let interaction = ScriptedInteraction::new(vec![Ok(String::new())]);
            let credential = flow(&origin)
                .login(&interaction, &LoginOptions::default())
                .await
                .unwrap();

            assert_eq!(policy_paths(&log), vec!["/models/gpt-5.5/policy"]);
            // (d) the union, deduplicated, in first-seen order: the picker ids then the newly
            // enabled one. `gpt-5.5` is already among the picker ids, so it must NOT appear twice.
            assert_eq!(
                available_ids(&credential),
                vec![
                    "gpt-5.5".to_string(),
                    "claude-sonnet-4.6".to_string(),
                    "not-in-our-catalog".to_string(),
                    // `claude-opus-4.1` is absent: `model_picker_enabled: false` keeps it out of
                    // the picker set, and the fallback does not apply while that set is non-empty.
                    "no-policy-state".to_string(),
                ]
            );
            assert!(
                interaction
                    .events()
                    .iter()
                    .any(|e| matches!(e, AuthEvent::Progress { .. })),
                "a non-empty policy list notifies (`:473`)"
            );
        }

        /// (d) the order-dependent half: a `/models` body that deliberately OMITS the id it reports
        /// as `unconfigured` from the available set. Upstream's union is the only way the enabled id
        /// reaches the credential, because the listing already happened.
        #[tokio::test]
        async fn a_newly_enabled_id_joins_available_model_ids_through_the_union() {
            // `gpt-5.5` is `unconfigured` AND not picker-enabled, with the fallback ON (the
            // individual token), so it is a policy target but not a picker id.
            let models = json!({"data": [
                {"id": "gpt-5.5", "model_picker_enabled": false,
                 "policy": {"state": "unconfigured"}},
            ]})
            .to_string();
            let (origin, log) = spawn_with_headers(Arc::new(move |path, nth| {
                if let Some((status, body)) = base_reply(path, nth, false) {
                    return (status, Vec::new(), body);
                }
                match path {
                    "/models" => (200, Vec::new(), models.clone()),
                    _ => (200, Vec::new(), "{}".to_string()),
                }
            }))
            .await;

            let interaction = ScriptedInteraction::new(vec![Ok(String::new())]);
            let credential = flow(&origin)
                .login(&interaction, &LoginOptions::default())
                .await
                .unwrap();
            assert_eq!(policy_paths(&log), vec!["/models/gpt-5.5/policy"]);
            assert_eq!(available_ids(&credential), vec!["gpt-5.5".to_string()]);
        }

        /// (c) `:154-164` — a 429 with `retry-after: 0` is RETRIED, and a 200 on the retry counts the
        /// id as enabled.
        #[tokio::test]
        async fn a_429_with_retry_after_zero_is_retried_and_then_succeeds() {
            let models = json!({"data": [
                {"id": "gpt-5.5", "model_picker_enabled": true,
                 "policy": {"state": "unconfigured"}},
            ]})
            .to_string();
            let (origin, log) = spawn_with_headers(Arc::new(move |path, nth| {
                if let Some((status, body)) = base_reply(path, nth, false) {
                    return (status, Vec::new(), body);
                }
                match path {
                    "/models" => (200, Vec::new(), models.clone()),
                    p if p.ends_with("/policy") && nth == 0 => (
                        429,
                        vec![("retry-after".to_string(), "0".to_string())],
                        "slow down".to_string(),
                    ),
                    _ => (200, Vec::new(), "{}".to_string()),
                }
            }))
            .await;

            let interaction = ScriptedInteraction::new(vec![Ok(String::new())]);
            let credential = flow(&origin)
                .login(&interaction, &LoginOptions::default())
                .await
                .unwrap();
            assert_eq!(
                policy_paths(&log),
                vec!["/models/gpt-5.5/policy", "/models/gpt-5.5/policy"],
                "the 429 was retried, not abandoned"
            );
            assert_eq!(available_ids(&credential), vec!["gpt-5.5".to_string()]);
        }

        /// (b) `:404-406` + `:420-424` — a 429 on EVERY attempt exhausts the budget, raises, and
        /// `break`s the batch: the second `unconfigured` id is never attempted. The login still
        /// succeeds and the credential still carries the listing's ids.
        #[tokio::test]
        async fn an_exhausted_rate_limit_stops_the_batch_without_failing_the_login() {
            let models = json!({"data": [
                {"id": "gpt-5.5", "model_picker_enabled": true,
                 "policy": {"state": "unconfigured"}},
                {"id": "claude-sonnet-4.6", "model_picker_enabled": true,
                 "policy": {"state": "unconfigured"}},
            ]})
            .to_string();
            let (origin, log) = spawn_with_headers(Arc::new(move |path, nth| {
                if let Some((status, body)) = base_reply(path, nth, false) {
                    return (status, Vec::new(), body);
                }
                match path {
                    "/models" => (200, Vec::new(), models.clone()),
                    p if p.ends_with("/policy") => {
                        let _ = nth;
                        (
                            429,
                            vec![("retry-after".to_string(), "0".to_string())],
                            "slow down".to_string(),
                        )
                    }
                    _ => (200, Vec::new(), "{}".to_string()),
                }
            }))
            .await;

            let interaction = ScriptedInteraction::new(vec![Ok(String::new())]);
            let credential = flow(&origin)
                .login(&interaction, &LoginOptions::default())
                .await
                .unwrap();

            // `maxRetries: 2` ⇒ the first attempt plus two retries, all for the FIRST id only.
            assert_eq!(
                policy_paths(&log),
                vec![
                    "/models/gpt-5.5/policy",
                    "/models/gpt-5.5/policy",
                    "/models/gpt-5.5/policy",
                ],
                "three attempts for the first id, then the batch breaks"
            );
            assert_eq!(
                available_ids(&credential),
                vec!["gpt-5.5".to_string(), "claude-sonnet-4.6".to_string()],
                "the listing's ids survive an entirely failed policy pass"
            );
        }

        /// (e) `:170-172` — `allowPolicyFallback` applies ONLY to
        /// `api.individual.githubcopilot.com`. The same body on an enterprise base url yields no
        /// available ids at all, because every row's picker flag is `false`.
        #[tokio::test]
        async fn the_policy_fallback_is_limited_to_the_individual_endpoint() {
            let body = json!({"data": [
                {"id": "gpt-5.5", "model_picker_enabled": false, "policy": {"state": "enabled"}},
                {"id": "claude-sonnet-4.6", "model_picker_enabled": false,
                 "policy": {"state": "enabled"}},
                {"id": "not-enabled", "model_picker_enabled": false,
                 "policy": {"state": "unconfigured"}},
            ]});

            for (enterprise, expected) in [
                (
                    false,
                    vec!["gpt-5.5".to_string(), "claude-sonnet-4.6".to_string()],
                ),
                (true, Vec::new()),
            ] {
                let models = body.to_string();
                let (origin, _log) = spawn_with_headers(Arc::new(move |path, nth| {
                    if let Some((status, reply)) = base_reply(path, nth, enterprise) {
                        return (status, Vec::new(), reply);
                    }
                    match path {
                        "/models" => (200, Vec::new(), models.clone()),
                        _ => (200, Vec::new(), "{}".to_string()),
                    }
                }))
                .await;

                let answer = if enterprise { "company.ghe.com" } else { "" };
                let interaction = ScriptedInteraction::new(vec![Ok(answer.to_string())]);
                let credential = flow(&origin)
                    .login(&interaction, &LoginOptions::default())
                    .await
                    .unwrap();
                assert_eq!(
                    available_ids(&credential),
                    expected,
                    "enterprise = {enterprise}"
                );
            }
        }

        /// A `/models` body whose single row is a policy target but NOT an available id: `gpt-5.5` is
        /// `unconfigured` and not picker-enabled, so with the individual token's fallback ON it is
        /// posted while `availableModelIds` comes only from the `enabled` rows (`:119-123`,
        /// `:124-132`). A successful POST therefore SHOWS UP in the credential through the union, and
        /// a failed one leaves `availableModelIds` empty — which is what makes an abort observable.
        fn one_policy_target_no_available_ids() -> String {
            json!({"data": [
                {"id": "gpt-5.5", "model_picker_enabled": false,
                 "policy": {"state": "unconfigured"}},
            ]})
            .to_string()
        }

        /// `:141-151` — the retry budget bounds the whole loop, IN-FLIGHT REQUESTS INCLUDED, not just
        /// the sleeps between them.
        ///
        /// pi arms `AbortSignal.timeout(maxElapsedMs)` (`:143`), composes it into `requestSignal`
        /// (`:145`) and hands that to EVERY `fetch` (`:148-151`). So an attempt that straddles the
        /// deadline is aborted mid-flight. Before PROV-098 cyrup only consulted the deadline when
        /// deciding whether the next SLEEP fit, and `send()` was bounded by nothing on this path:
        /// a policy endpoint that accepted the connection and then went quiet held the login for the
        /// five-MINUTE read-idle timeout of `stream/sse.rs:66`, with `COPILOT_MAX_RATE_LIMIT_ELAPSED_MS`
        /// powerless to stop it.
        ///
        /// The abort is pi's composed-signal abort, where the CALLER's `signal.aborted` is still
        /// false, so `enableGitHubCopilotModel`'s `catch` (`:400-403`) yields `false` and the login
        /// still succeeds — it just enables nothing.
        #[tokio::test]
        async fn an_attempt_that_outlives_the_retry_budget_fails_like_pis_abort() {
            let models = one_policy_target_no_available_ids();
            let (origin, log) = spawn_with_headers(Arc::new(move |path, nth| {
                if let Some((status, reply)) = base_reply(path, nth, false) {
                    return (status, Vec::new(), reply);
                }
                match path {
                    "/models" => (200, Vec::new(), models.clone()),
                    // First policy POST: rate-limited with a zero delay, so the loop retries at once.
                    p if p.ends_with("/policy") && nth == 0 => (
                        429,
                        vec![("retry-after".to_string(), "0".to_string())],
                        "slow down".to_string(),
                    ),
                    // The retry never gets an answer. Only the budget can end it.
                    p if p.ends_with("/policy") => (0, Vec::new(), String::new()),
                    _ => (200, Vec::new(), "{}".to_string()),
                }
            }))
            .await;

            let interaction = ScriptedInteraction::new(vec![Ok(String::new())]);
            // `attempt_timeout` is deliberately far LONGER than the budget: the only thing that can
            // end the stalled retry is `max_elapsed_ms`, so this isolates the budget from the
            // per-attempt cap.
            let flow = flow(&origin).with_retry_policy(CopilotRetryPolicy {
                max_retries: 2,
                max_elapsed_ms: 400,
                attempt_timeout: Duration::from_secs(10),
            });
            // 4s is far above the ~2.2s the scripted device flow itself costs and far below the
            // five MINUTES an unbounded stall would take (`stream/sse.rs:66`), so what this wrapper
            // measures is the budget and nothing else.
            let credential = tokio::time::timeout(
                Duration::from_secs(4),
                flow.login(&interaction, &LoginOptions::default()),
            )
            .await
            .expect("the retry budget must end the stalled attempt")
            .expect("an abort inside the batch must not fail the login");

            // Two attempts: the 429 and the stalled retry. `max_retries: 2` would allow a third, but
            // the budget is gone before it can be issued.
            assert_eq!(
                policy_paths(&log),
                vec![
                    "/models/gpt-5.5/policy".to_string(),
                    "/models/gpt-5.5/policy".to_string()
                ]
            );
            // The POST never succeeded, so the union adds nothing.
            assert!(available_ids(&credential).is_empty());
        }

        /// `:150` — every attempt runs under its own `AbortSignal.timeout(5000)`, the policy POST
        /// included.
        ///
        /// This is pi's `{maxRetries: 0, maxElapsedMs: 0}` shape (`:359-362`), where `:141-144` leaves
        /// the budget signal unarmed and the per-attempt cap is the ONLY bound left. cyrup used to
        /// set `.timeout(...)` on the `/models` GET alone (`fetch_model_catalog`), leaving
        /// `/models/{id}/policy` with no deadline of its own at all.
        #[tokio::test]
        async fn the_policy_post_is_capped_per_attempt() {
            let models = json!({"data": [
                {"id": "gpt-5.5", "model_picker_enabled": false,
                 "policy": {"state": "unconfigured"}},
                {"id": "claude-sonnet-4.6", "model_picker_enabled": false,
                 "policy": {"state": "enabled"}},
            ]})
            .to_string();
            let (origin, log) = spawn_with_headers(Arc::new(move |path, nth| {
                if let Some((status, reply)) = base_reply(path, nth, false) {
                    return (status, Vec::new(), reply);
                }
                match path {
                    "/models" => (200, Vec::new(), models.clone()),
                    p if p.ends_with("/policy") => (0, Vec::new(), String::new()),
                    _ => (200, Vec::new(), "{}".to_string()),
                }
            }))
            .await;

            let interaction = ScriptedInteraction::new(vec![Ok(String::new())]);
            let flow = flow(&origin).with_retry_policy(CopilotRetryPolicy {
                max_retries: 0,
                max_elapsed_ms: 0,
                attempt_timeout: Duration::from_millis(120),
            });
            // Same margin as the budget test: well above the scripted device flow's own ~2.2s,
            // far below an unbounded stall's five minutes.
            let credential = tokio::time::timeout(
                Duration::from_secs(4),
                flow.login(&interaction, &LoginOptions::default()),
            )
            .await
            .expect("the per-attempt cap must end the stalled POST")
            .expect("an abort inside the batch must not fail the login");

            // `max_retries: 0` means `retry == policy.max_retries` on the first pass, so there is
            // exactly one attempt — and it was capped.
            assert_eq!(
                policy_paths(&log),
                vec!["/models/gpt-5.5/policy".to_string()]
            );
            // The stalled id never joins the union; the already-`enabled` row still does.
            assert_eq!(
                available_ids(&credential),
                vec!["claude-sonnet-4.6".to_string()]
            );
        }
    }

    /// `:337-339` — a non-empty input that does not normalize to a host is fatal, and no request
    /// is made. The MIRROR half proves a *valid* enterprise domain is accepted by the same code.
    #[tokio::test]
    async fn unparseable_enterprise_domain_is_rejected_before_any_request() {
        let (origin, log) = spawn(Arc::new(|_, _| (200, "{}".to_string()))).await;
        let interaction = ScriptedInteraction::new(vec![Ok("://".to_string())]);
        let err = flow(&origin)
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Invalid GitHub Enterprise URL/domain");
        assert!(log.lock().unwrap().is_empty(), "no network call was made");

        // MIRROR: the same field with a real domain gets past the guard and starts the flow.
        assert_eq!(
            normalize_domain("https://company.ghe.com/").as_deref(),
            Some("company.ghe.com")
        );
    }

    /// An enterprise login records the domain on the credential (`:275`), which is what
    /// `copilotEnterpriseDomain` (`:361-365`) reads back on refresh.
    #[tokio::test]
    async fn enterprise_login_records_the_domain_on_the_credential() {
        let device = device_code_body().to_string();
        let token = copilot_token_body().to_string();
        let models = json!({"data": []}).to_string();
        let (origin, _) = spawn(Arc::new(move |path, _| match path {
            "/login/device/code" => (200, device.clone()),
            "/login/oauth/access_token" => (200, json!({"access_token": "gho_ent"}).to_string()),
            "/copilot_internal/v2/token" => (200, token.clone()),
            "/models" => (200, models.clone()),
            _ => (200, "{}".to_string()),
        }))
        .await;

        let interaction =
            ScriptedInteraction::new(vec![Ok("  https://Company.GHE.com/x  ".into())]);
        let credential = flow(&origin)
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap();
        let Credential::Oauth { ext, .. } = credential else {
            panic!("expected an OAuth credential");
        };
        assert_eq!(
            ext.get("enterpriseUrl").and_then(Value::as_str),
            Some("company.ghe.com")
        );
        assert_eq!(ext.get("availableModelIds").unwrap(), &json!([]));
    }

    /// `:335` — a login aborted while the user was answering the domain prompt reports upstream's
    /// `CANCEL_MESSAGE` and issues no request.
    #[tokio::test]
    async fn abort_after_the_prompt_reports_login_cancelled() {
        let (origin, log) = spawn(Arc::new(|_, _| (200, "{}".to_string()))).await;
        let cancel = CancelToken::new();
        cancel.cancel();
        let interaction =
            ScriptedInteraction::new(vec![Ok(String::new())]).with_cancel(cancel.clone());
        let err = flow(&origin)
            .login(&interaction, &LoginOptions::default())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Login cancelled");
        assert!(log.lock().unwrap().is_empty());
    }

    /// The strategy object (`github-copilot.ts:367-379`): the name is upstream's, and `to_auth`
    /// is the provider module's port, reached through this flow — so registering this one object
    /// serves all three members.
    #[tokio::test]
    async fn strategy_delegates_to_auth_to_the_runtime_half() {
        let flow = GitHubCopilotLogin::new();
        assert_eq!(flow.name(), "GitHub Copilot");
        assert_eq!(provider_id(), "github-copilot");

        let cred = Credential::Oauth {
            refresh: "gho_user".into(),
            access: "tid=x;proxy-ep=proxy.enterprise.githubcopilot.com;".into(),
            expires: 0,
            ext: Map::new(),
        };
        let auth = flow.to_auth(&cred).await.unwrap();
        assert_eq!(
            auth.base_url.as_deref(),
            Some("https://api.enterprise.githubcopilot.com"),
            "the proxy-ep claim drives the request endpoint (`:373-377`)"
        );
        assert!(auth.api_key.is_some());
    }
}

#[cfg(test)]
mod upstream_normalization_tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::parse_http_href;

    /// pi `packages/ai/test/github-copilot-oauth.test.ts`, case "normalizes verification_uri before
    /// it reaches onDeviceCode". The raw value carries an OSC-8 hyperlink introducer
    /// (`\u{1b}]8;;`); WHATWG percent-encodes C0 controls in the path, so `href != raw` and the RAW
    /// value must never reach the user-facing device-code prompt. Without the encoding the escape
    /// reaches the terminal, where it can retarget the very link the user is being told to open.
    #[test]
    fn normalizes_verification_uri_before_it_reaches_the_user() {
        let raw = "https://github.com/login/\u{1b}]8;;evil";
        let href = parse_http_href(raw).expect("an https URI parses");

        assert_ne!(
            href, raw,
            "href must differ from the raw input (upstream asserts exactly this)"
        );
        assert_eq!(href, "https://github.com/login/%1B]8;;evil");
        assert!(
            !href.contains('\u{1b}'),
            "no C0 control survives into the prompt"
        );
    }

    /// MIRROR: an ordinary URI is untouched, so the assertion above is about the escape and not
    /// about the encoder mangling every input.
    #[test]
    fn an_ordinary_verification_uri_is_unchanged_apart_from_whatwg_defaults() {
        assert_eq!(
            parse_http_href("https://github.com/login/device").unwrap(),
            "https://github.com/login/device"
        );
        // `new URL("https://host").href === "https://host/"`
        assert_eq!(
            parse_http_href("https://github.com").unwrap(),
            "https://github.com/"
        );
    }
}
