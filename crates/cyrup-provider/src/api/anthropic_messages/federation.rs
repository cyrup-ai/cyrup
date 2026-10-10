//! Anthropic workload identity federation (PROV-119).
//!
//! A workload that already holds an OIDC identity token from a trusted issuer (an EKS/GKE
//! projected service-account token, a GitHub Actions JWT, a SPIFFE SVID) exchanges it at
//! `POST /v1/oauth/token` for a short-lived Anthropic access token, so no `sk-ant-api…` key is
//! ever minted, distributed or rotated.
//!
//! # Where this diverges from pi, and why
//!
//! pi resolves the federation ids and hands them to the **Anthropic SDK** as a `config`
//! (`getAnthropicFederation`, `api/anthropic-messages.ts:349-371` @v1.0.1); the SDK performs the
//! exchange, the refresh and the token caching. cyrup has no Anthropic SDK, so all of that lives
//! here, which is what `PROV-119`'s Fix calls "the real work". Two consequences:
//!
//! 1. **The activation gate is the documented one, not pi's.** pi activates federation on three
//!    variables — rule id, organization id, identity-token file (`providers/anthropic.ts:54-63`)
//!    — and treats `ANTHROPIC_SERVICE_ACCOUNT_ID` as optional. But `service_account_id` is a
//!    REQUIRED field of the exchange request, and the SDK's own activation set is four variables
//!    including it (<https://platform.claude.com/docs/en/manage-claude/wif-reference>). pi gets
//!    away with the looser gate because the SDK validates the config it is handed; cyrup builds
//!    the request itself, so porting pi's gate verbatim would activate federation and then fail
//!    the exchange with the deliberately opaque `401 Authentication failed`, with the real reason
//!    visible only in the Console's authentication history. CYRUP-DELTA, recorded on
//!    [`FederationConfig::from_env`].
//! 2. **The minted token never enters `ModelAuth::api_key`.** A federated access token is
//!    prefixed `sk-ant-oat01-…`, and [`super::claude_code::is_oauth_token`] is
//!    `api_key.contains("sk-ant-oat")` — so a token placed in the key slot would flip `is_oauth`
//!    and silently rewrite every tool name to its Claude Code alias on the wire
//!    (`convert.rs`, `messages.rs`, `events.rs`). pi avoids this structurally by returning
//!    `isOAuthToken: false` for the federation client (`anthropic-messages.ts:1066`); cyrup avoids
//!    it the same way, by carrying the token as an `Authorization: Bearer` header overlay exactly
//!    as the `ANTHROPIC_AUTH_TOKEN` arm does. [`apply_to_auth`] is the only writer.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::auth::{AuthResult, ModelAuth};
use crate::env_api_keys::{
    ANTHROPIC_FEDERATION_RULE_ID_ENV, ANTHROPIC_IDENTITY_TOKEN_ENV,
    ANTHROPIC_IDENTITY_TOKEN_FILE_ENV, ANTHROPIC_ORGANIZATION_ID_ENV,
    ANTHROPIC_SERVICE_ACCOUNT_ID_ENV, ANTHROPIC_WORKSPACE_ID_ENV,
};
use crate::model::Model;
use crate::utils::provider_plumbing::EnvSource;

/// RFC 7523 `jwt-bearer` grant, the only grant this endpoint accepts for federation.
const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// Path of the exchange endpoint, appended to the model's base URL.
const TOKEN_PATH: &str = "/v1/oauth/token";

/// Re-exchange this long before the token's stated expiry, so an in-flight request cannot be
/// answered with a token that expires between the header being written and the server reading it.
///
/// The rule's `token_lifetime_seconds` may be as low as 60, so a fixed 60 s margin would re-exchange
/// on every single request for a short-lived rule — and because an `assertion` carrying a `jti` may
/// be exchanged only ONCE per issuer, a hot re-exchange loop is not merely wasteful, it is how a
/// workload starts failing with `jti_reused`. The margin is therefore the smaller of 60 s and a
/// tenth of the token's own lifetime.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// Where the IdP-issued JWT comes from. Both forms are documented as alternatives of equal
/// standing; pi reads only the file (see the module header's CYRUP-DELTA).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum IdentityToken {
    /// `ANTHROPIC_IDENTITY_TOKEN_FILE` — a path, re-read on every exchange.
    File(String),
    /// `ANTHROPIC_IDENTITY_TOKEN` — the literal JWT.
    Inline(String),
}

/// A resolved federation configuration: everything the exchange needs except the assertion, which
/// is read fresh each time.
///
/// `Hash`/`Eq` make this the cache key together with the base URL, mirroring pi's
/// `JSON.stringify([model.baseUrl, federation])` (`anthropic-messages.ts:1053`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct FederationConfig {
    federation_rule_id: String,
    organization_id: String,
    service_account_id: String,
    /// Optional, and it does NOT gate activation: required only when the rule is enabled for more
    /// than one workspace, in which case omitting it denies the exchange with the history reason
    /// `workspace_id_required`.
    workspace_id: Option<String>,
    identity_token: IdentityToken,
}

impl FederationConfig {
    /// Resolve the federation configuration for this request, or `None` when federation does not
    /// apply.
    ///
    /// Three gates, in pi's order (`getAnthropicFederation`, `anthropic-messages.ts:355-359`):
    ///
    /// 1. **The anthropic provider only** — the exchange is an Anthropic API endpoint, so an
    ///    Anthropic-compatible gateway on another provider id must not be sent these ids.
    /// 2. **No request auth already resolved** — federation is last in the credential chain, so a
    ///    key, a bearer token or an auth header wins. This is `hasRequestAuth` and it is why an
    ///    `ANTHROPIC_API_KEY=""` exported as an empty string is a documented foot-gun upstream:
    ///    cyrup's own resolver treats an empty value as absent (`get_provider_env_value`), so the
    ///    empty-string trap does not reproduce here.
    /// 3. **The four required variables present** — CYRUP-DELTA against pi's three; see the module
    ///    header. `service_account_id` is a required field of the request body, so activating
    ///    without it would only produce an opaque 401.
    ///
    /// `env` is an [`EnvSource`] rather than a bare `Option<&ProviderEnv>` deliberately. That type
    /// can say "this variable is X" but never "this variable is ABSENT", so an empty overlay falls
    /// through to `std::env::var`, and a test asserting that federation does NOT activate would
    /// silently inherit the shell of whoever runs it.
    pub(super) fn from_env(model: &Model, auth: &AuthResult, env: EnvSource<'_>) -> Option<Self> {
        if model.provider.as_str() != "anthropic" {
            return None;
        }
        if has_request_auth(auth) {
            return None;
        }
        // The scoped overlay first, then the ambient environment, with an empty value counting as
        // absent — pi's `getProviderEnvValue` precedence.
        let get = |name: &str| env.get(name);

        let federation_rule_id = get(ANTHROPIC_FEDERATION_RULE_ID_ENV)?;
        let organization_id = get(ANTHROPIC_ORGANIZATION_ID_ENV)?;
        let service_account_id = get(ANTHROPIC_SERVICE_ACCOUNT_ID_ENV)?;
        // `_FILE` first: when both are set, a rotating projected file is the fresher source, and it
        // is the one pi supports.
        let identity_token = match get(ANTHROPIC_IDENTITY_TOKEN_FILE_ENV) {
            Some(path) => IdentityToken::File(path),
            None => IdentityToken::Inline(get(ANTHROPIC_IDENTITY_TOKEN_ENV)?),
        };
        Some(Self {
            federation_rule_id,
            organization_id,
            service_account_id,
            workspace_id: get(ANTHROPIC_WORKSPACE_ID_ENV),
            identity_token,
        })
    }

    /// The exchange request body. `workspace_id` is omitted entirely when absent rather than sent
    /// as `null`: the server selects the rule's sole enabled workspace in that case, and a literal
    /// `null` is not one of the documented values.
    fn exchange_body(&self, assertion: &str) -> serde_json::Value {
        let mut body = serde_json::json!({
            "grant_type": JWT_BEARER_GRANT,
            "assertion": assertion,
            "federation_rule_id": self.federation_rule_id,
            "organization_id": self.organization_id,
            "service_account_id": self.service_account_id,
        });
        if let Some(workspace) = &self.workspace_id
            && let Some(obj) = body.as_object_mut()
        {
            obj.insert(
                "workspace_id".to_string(),
                serde_json::Value::String(workspace.clone()),
            );
        }
        body
    }

    /// Read the assertion. The file is re-read on EVERY exchange, which is both what the SDK
    /// documents and what correctness requires: a projected token rotates on disk, and an
    /// assertion carrying a `jti` may be exchanged only once per issuer, so re-sending a cached
    /// one is rejected as a replay.
    async fn read_assertion(&self) -> Result<String, FederationError> {
        match &self.identity_token {
            IdentityToken::Inline(jwt) => Ok(jwt.clone()),
            IdentityToken::File(path) => tokio::fs::read_to_string(path)
                .await
                .map(|s| s.trim().to_string())
                .map_err(|e| FederationError::IdentityToken {
                    path: path.clone(),
                    source: e.to_string(),
                }),
        }
    }
}

/// `hasRequestAuth` (pi `anthropic-messages.ts:324-326`): a key or ANY auth header already
/// resolved. Split out of the assertion so the assertion can be skipped when federation applies.
///
/// The header test is case-insensitive because a header overlay is a plain map: pi's `headers`
/// object is keyed however the producer wrote it, and `Authorization` vs `authorization` must not
/// decide whether a workload federates.
fn has_request_auth(auth: &AuthResult) -> bool {
    if auth.auth.api_key.as_ref().is_some_and(|k| !k.is_empty()) {
        return true;
    }
    auth.auth.headers.as_ref().is_some_and(|h| {
        h.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization")
                && value.as_ref().is_some_and(|v| !v.is_empty())
        })
    })
}

/// Why an exchange could not produce a token. Every variant is terminal for the request: there is
/// no key to fall back to, which is the point of federation.
#[derive(Debug)]
pub(super) enum FederationError {
    IdentityToken { path: String, source: String },
    Transport(String),
    Status { status: u16, body: String },
    Malformed(String),
}

impl std::fmt::Display for FederationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IdentityToken { path, source } => write!(
                f,
                "workload identity federation: cannot read the identity token at {path}: {source}"
            ),
            Self::Transport(e) => write!(
                f,
                "workload identity federation: token exchange failed: {e}"
            ),
            // The 401 body is deliberately opaque upstream (always `Authentication failed`), so the
            // message points at where the real reason IS recorded instead of pretending to know it.
            Self::Status { status, body } => write!(
                f,
                "workload identity federation: token exchange returned {status}: {body}. A 401 is \
                 always reported as `Authentication failed`; the deny reason is recorded in the \
                 Console's authentication history for this federation rule."
            ),
            Self::Malformed(e) => {
                write!(
                    f,
                    "workload identity federation: malformed token response: {e}"
                )
            }
        }
    }
}

/// A minted token and the instant it stops being usable.
#[derive(Clone, Debug)]
struct CachedToken {
    access_token: String,
    /// Already margin-adjusted: the token is re-exchanged once `SystemTime::now()` passes this.
    refresh_at: SystemTime,
}

/// The process-wide token cache, keyed by `(base_url, config)`.
///
/// pi keeps ONE client and clones it per request so the SDK's own token cache survives
/// (`anthropic-messages.ts:1049-1067`); the cache is the part that matters, so cyrup caches the
/// token directly. A `std::sync::Mutex` is held only to read or replace a small struct — never
/// across the exchange await — so it cannot block the runtime.
fn cache() -> &'static Mutex<HashMap<String, CachedToken>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedToken>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Mirrors pi's `JSON.stringify([model.baseUrl, federation])` cache key.
fn cache_key(base_url: &str, config: &FederationConfig) -> String {
    format!("{base_url}\u{0}{config:?}")
}

/// The token-exchange URL for a model's API base.
///
/// A model's `base_url` may already BE the `/v1/messages` endpoint (`messages_url` is idempotent
/// for exactly that reason), so the suffix is stripped before the exchange path is appended —
/// otherwise a catalog row carrying the full endpoint would POST to
/// `…/v1/messages/v1/oauth/token`.
fn token_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let base = trimmed.strip_suffix("/v1/messages").unwrap_or(trimmed);
    format!("{base}{TOKEN_PATH}")
}

/// Mint or reuse an access token for this configuration.
///
/// The cached token is reused until it is within [`REFRESH_MARGIN`] (or a tenth of its lifetime,
/// whichever is smaller) of expiry.
pub(super) async fn access_token(
    base_url: &str,
    config: &FederationConfig,
) -> Result<String, FederationError> {
    let key = cache_key(base_url, config);
    if let Ok(guard) = cache().lock()
        && let Some(entry) = guard.get(&key)
        && SystemTime::now() < entry.refresh_at
    {
        return Ok(entry.access_token.clone());
    }

    let assertion = config.read_assertion().await?;
    let url = token_url(base_url);
    // PROV-047: the proxy-aware builder, so a federated exchange honours `httpProxy` like every
    // other outbound call. A bare `reqwest::Client` here would bypass a configured proxy.
    let client = crate::stream::sse::build_client_for_target(
        &url,
        &crate::auth::types::EnvAuthContext,
        None,
        None,
    )
    .await
    .map_err(|e| FederationError::Transport(e.to_string()))?;

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(&config.exchange_body(&assertion))
        .send()
        .await
        .map_err(|e| FederationError::Transport(e.to_string()))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| FederationError::Transport(e.to_string()))?;
    if !status.is_success() {
        return Err(FederationError::Status {
            status: status.as_u16(),
            body,
        });
    }

    let (access_token, expires_in) = parse_token_response(&body)?;
    let margin = REFRESH_MARGIN.min(Duration::from_secs(expires_in / 10));
    let refresh_at = SystemTime::now() + Duration::from_secs(expires_in).saturating_sub(margin);
    if let Ok(mut guard) = cache().lock() {
        guard.insert(
            key,
            CachedToken {
                access_token: access_token.clone(),
                refresh_at,
            },
        );
    }
    Ok(access_token)
}

/// Pull `access_token` and `expires_in` out of an RFC 6749 §5.1 token response.
///
/// `token_type` is documented as always `Bearer` and is not checked: a server that answered with
/// another type would still be sent `Authorization: Bearer`, and failing the request over a field
/// that cannot vary would only turn a working exchange into an outage.
fn parse_token_response(body: &str) -> Result<(String, u64), FederationError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| FederationError::Malformed(e.to_string()))?;
    let access_token = value
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| FederationError::Malformed("no `access_token` in the response".to_string()))?
        .to_string();
    // `expires_in` is documented as present; a response without it is treated as a single-use
    // token (refreshed on the next request) rather than cached forever.
    let expires_in = value
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    Ok((access_token, expires_in))
}

/// Layer the minted token onto the request's auth as an `Authorization: Bearer` header.
///
/// This is the ONLY place a federated token is written, and it deliberately does not touch
/// `api_key`: see the module header for what `is_oauth` would do to the wire if it did. Any
/// existing `Authorization` header would have suppressed federation in
/// [`FederationConfig::from_env`], so there is nothing here to overwrite.
pub(super) fn apply_to_auth(auth: &AuthResult, token: &str) -> AuthResult {
    let mut headers = auth.auth.headers.clone().unwrap_or_default();
    headers.insert("Authorization".to_string(), Some(format!("Bearer {token}")));
    AuthResult {
        auth: ModelAuth {
            api_key: None,
            headers: Some(headers),
            base_url: auth.auth.base_url.clone(),
        },
        env: auth.env.clone(),
        source: Some("workload identity federation".to_string()),
    }
}

#[cfg(test)]
mod tests;
