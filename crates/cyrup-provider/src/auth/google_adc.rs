//! Google Application Default Credentials → OAuth 2.0 access token (PROV-030).
//!
//! # Why this module exists, and what it is a port *of*
//!
//! pi's Vertex adapter never mints a bearer itself. `api/google-vertex.ts:327-339` @v0.83.0
//! constructs `new GoogleGenAI({ vertexai: true, project, location, apiVersion, googleAuthOptions })`
//! and hands the whole credential problem to `@google/genai`, which delegates to
//! `google-auth-library`'s `GoogleAuth`. The only pi-side input is `buildGoogleAuthOptions`
//! (`google-vertex.ts:383-386`), which sets `{ keyFilename }` **iff**
//! `getProviderEnvValue("GOOGLE_APPLICATION_CREDENTIALS", env)` is set — otherwise the library falls
//! back to its own Application Default Credentials search.
//!
//! cyrup has no SDK to delegate to, so the library's behaviour is what has to be reproduced. That
//! makes this the one place in the Vertex port where "cite the pi file:line" cannot be satisfied by
//! a pi source line: the mechanism lives one dependency down. Everything below therefore states
//! which `google-auth-library` behaviour it reproduces, and the divergences are enumerated
//! explicitly rather than left implicit.
//!
//! ## `[CYRUP-DELTA]` — the credential types this module does NOT accept
//!
//! `google-auth-library`'s `GoogleAuth.fromJSON` dispatches on the credentials file's `type` field
//! and accepts five: `authorized_user`, `service_account`, `external_account`,
//! `impersonated_service_account` and `gdch_service_account`. This module implements the first two
//! plus the GCE metadata-server fallback, and rejects the other three with a message naming the
//! type (see [`AdcCredentials::from_json`]).
//!
//! That is a deliberate, bounded scope rather than a silent one, and it is chosen to cover exactly
//! the credentials cyrup itself can produce or a user can be told to produce:
//!
//! * `authorized_user` — what `gcloud auth application-default login` writes, i.e. the arm
//!   `GoogleVertexApiKeyAuth::login`'s `"adc"` option instructs the user to run
//!   (`providers/google_vertex.rs` `ADC_INFO_MESSAGE`, pi `google-vertex.ts:38-41`).
//! * `service_account` — what the `"service-account"` login arm stores a path to
//!   (`google-vertex.ts:50-52` → `GOOGLE_APPLICATION_CREDENTIALS`).
//! * GCE metadata — the ambient case on a GCP VM / Cloud Run, where no file exists at all.
//!
//! `external_account` (workload-identity federation) and `impersonated_service_account` are
//! multi-hop credential chains with their own executable/URL/AWS subject-token sources; porting them
//! without a user who has one is speculative. They fail loudly here, naming the type and this note.
//!
//! ## What IS reproduced faithfully
//!
//! * The search order: the `GOOGLE_APPLICATION_CREDENTIALS` path (scoped overlay first, then the
//!   process env — pi `getProviderEnvValue`, `provider-env.ts:44-52`), then the well-known
//!   `gcloud` file, then the metadata server.
//! * The `authorized_user` refresh-token exchange: `POST {token_uri}` as
//!   `application/x-www-form-urlencoded` with `client_id`/`client_secret`/`refresh_token`/
//!   `grant_type=refresh_token`.
//! * The `service_account` self-signed JWT-bearer exchange: an RS256 assertion over
//!   `{iss, scope, aud, exp, iat}` with a one-hour lifetime, posted as
//!   `grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer`.
//! * `cloud-platform` as the requested scope — the scope `@google/genai` asks `GoogleAuth` for in
//!   Vertex mode.
//! * Token caching with an *eager* refresh window: the library treats a token as expired
//!   `eagerRefreshThresholdMillis` (5 minutes) BEFORE its real expiry, so a request never starts
//!   with a token that will die mid-stream. Reproduced by [`EAGER_REFRESH_THRESHOLD_MS`].
//!
//! The outbound client is built through [`crate::stream::sse::build_client_for_target`], so the
//! ported proxy resolver (`node-http-proxy.ts:92-112`) and the `httpProxy` setting
//! ([`crate::stream::sse::configure_http_proxy`]) govern token minting too, as PROV-047 requires of
//! every egress path.

use crate::auth::types::{AuthContext, ProviderEnv};
use crate::error::ProviderError;
use base64::Engine as _;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The OAuth scope `@google/genai` requests for Vertex traffic.
pub const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// The default `token_uri` for both grant types. A credentials file may override it with its own
/// `token_uri` member (service-account files always carry one).
pub const GOOGLE_OAUTH2_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

/// The GCE / Cloud Run metadata-server token endpoint, used when no credentials file is found.
pub const GCE_METADATA_TOKEN_URL: &str =
    "http://metadata.google.internal/computeMetadata/v1/instance/service-account/default/token";

/// The `grant_type` for a self-signed service-account JWT assertion (RFC 7523 §2.1).
pub const JWT_BEARER_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";

/// Lifetime of the signed assertion, in seconds. `google-auth-library` uses one hour, which is also
/// the maximum Google's token endpoint accepts.
pub const ASSERTION_LIFETIME_SECS: i64 = 3600;

/// `google-auth-library`'s `eagerRefreshThresholdMillis` default: a cached token is treated as
/// expired five minutes early, so a stream never opens on a token that expires mid-flight.
pub const EAGER_REFRESH_THRESHOLD_MS: i64 = 300_000;

/// The well-known ADC file `gcloud auth application-default login` writes on Unix. Identical to
/// [`crate::providers::google_vertex::VERTEX_ADC_PATH`], which pi hands to `ctx.fileExists`
/// (`google-vertex.ts:6`); kept as its own constant because this module *reads* the file rather
/// than testing for it, and must therefore expand `~` itself.
pub const ADC_WELL_KNOWN_UNIX: &str = "~/.config/gcloud/application_default_credentials.json";

/// The Windows well-known location (`%APPDATA%\gcloud\...`), the other half of the library's
/// well-known search.
pub const ADC_WELL_KNOWN_WINDOWS_SUFFIX: &str = "gcloud/application_default_credentials.json";

/// The env var naming an explicit credentials file (pi `google-vertex.ts:383-386`).
pub const GOOGLE_APPLICATION_CREDENTIALS_ENV: &str = "GOOGLE_APPLICATION_CREDENTIALS";

// ---------------------------------------------------------------------------
// Token cache
// ---------------------------------------------------------------------------

/// A minted access token plus its absolute expiry, in Unix **milliseconds** — the same unit
/// [`crate::auth::types::Credential::Oauth::expires`] uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdcToken {
    pub access_token: String,
    pub expires_at_ms: i64,
}

impl AdcToken {
    /// `google-auth-library`'s eager-expiry predicate: expired `EAGER_REFRESH_THRESHOLD_MS` before
    /// the real deadline.
    pub fn is_fresh_at(&self, now_ms: i64) -> bool {
        now_ms + EAGER_REFRESH_THRESHOLD_MS < self.expires_at_ms
    }
}

/// Process-global cache, keyed by credential *source* (the resolved file path, or the metadata
/// sentinel). `GoogleAuth` caches on the client object; cyrup builds an `ApiImpl` per api id and a
/// fresh `AuthResult` per request, so the cache has to outlive both or every turn would pay a token
/// round-trip.
///
/// A `std::sync::Mutex` is correct here and deliberate: nothing is `await`ed while the guard is
/// held. The get-then-mint sequence is intentionally NOT atomic — two concurrent first requests may
/// both mint, and the second write wins. That races only on wasted work, never on correctness, and
/// avoids holding a lock across an HTTP round trip.
fn cache() -> &'static Mutex<HashMap<String, AdcToken>> {
    static CACHE: OnceLock<Mutex<HashMap<String, AdcToken>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_get(key: &str, now_ms: i64) -> Option<AdcToken> {
    let guard = cache().lock().ok()?;
    guard.get(key).filter(|t| t.is_fresh_at(now_ms)).cloned()
}

fn cache_put(key: &str, token: &AdcToken) {
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key.to_string(), token.clone());
    }
}

/// Drop every cached token. Test-only: the cache is process-global, so a test that mints against a
/// loopback server must not be able to observe another test's token.
#[cfg(test)]
pub(crate) fn clear_cache() {
    if let Ok(mut guard) = cache().lock() {
        guard.clear();
    }
}

// ---------------------------------------------------------------------------
// Credential source resolution
// ---------------------------------------------------------------------------

/// Where the credentials come from — the ordered search `GoogleAuth.getClient()` performs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdcSource {
    /// An explicit or well-known credentials JSON file.
    File(String),
    /// No file found; fall back to the GCE / Cloud Run metadata server.
    Metadata,
}

impl AdcSource {
    /// The cache key for this source.
    fn cache_key(&self) -> String {
        match self {
            AdcSource::File(path) => format!("file:{path}"),
            AdcSource::Metadata => "metadata:default".to_string(),
        }
    }
}

/// Expand a leading `~` against `$HOME` (`$USERPROFILE` on Windows), the expansion pi's
/// `defaultProviderAuthContext.fileExists` performs before `fs.access` (`auth/context.ts:29-33`).
/// A path with no leading `~`, or a `~` with no home to expand against, is returned unchanged.
pub async fn expand_home(path: &str, ctx: &dyn AuthContext) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_string();
    };
    let home = match ctx.env("HOME").await {
        Some(h) if !h.is_empty() => Some(h),
        _ => ctx.env("USERPROFILE").await.filter(|h| !h.is_empty()),
    };
    match home {
        Some(home) => format!("{home}{rest}"),
        None => path.to_string(),
    }
}

/// pi `getProviderEnvValue(name, env)` (`provider-env.ts:44-52`): the provider-scoped overlay wins
/// over the ambient environment, and an empty string counts as absent (JS `||`).
fn provider_env_value(name: &str, env: Option<&ProviderEnv>, ambient: Option<String>) -> Option<String> {
    if let Some(map) = env
        && let Some(v) = map.get(name).filter(|v| !v.is_empty())
    {
        return Some(v.clone());
    }
    ambient.filter(|v| !v.is_empty())
}

/// Resolve which credential source to use, in `GoogleAuth`'s order:
/// `GOOGLE_APPLICATION_CREDENTIALS` → the well-known `gcloud` file → the metadata server.
pub async fn resolve_source(ctx: &dyn AuthContext, env: Option<&ProviderEnv>) -> AdcSource {
    let ambient = ctx.env(GOOGLE_APPLICATION_CREDENTIALS_ENV).await;
    if let Some(explicit) = provider_env_value(GOOGLE_APPLICATION_CREDENTIALS_ENV, env, ambient) {
        return AdcSource::File(expand_home(&explicit, ctx).await);
    }

    let unix = expand_home(ADC_WELL_KNOWN_UNIX, ctx).await;
    if ctx.file_exists(&unix).await {
        return AdcSource::File(unix);
    }

    if let Some(appdata) = ctx.env("APPDATA").await.filter(|v| !v.is_empty()) {
        let windows = format!(
            "{}/{}",
            appdata.trim_end_matches(['/', '\\']),
            ADC_WELL_KNOWN_WINDOWS_SUFFIX
        );
        if ctx.file_exists(&windows).await {
            return AdcSource::File(windows);
        }
    }

    AdcSource::Metadata
}

// ---------------------------------------------------------------------------
// Credentials file
// ---------------------------------------------------------------------------

/// A parsed ADC credentials file, narrowed to the two types this module mints from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdcCredentials {
    /// `type: "authorized_user"` — what `gcloud auth application-default login` writes.
    AuthorizedUser {
        client_id: String,
        client_secret: String,
        refresh_token: String,
        token_uri: String,
    },
    /// `type: "service_account"` — a downloaded service-account key file.
    ServiceAccount {
        client_email: String,
        private_key: String,
        private_key_id: Option<String>,
        token_uri: String,
    },
}

impl AdcCredentials {
    /// Parse a credentials JSON document, reproducing `GoogleAuth.fromJSON`'s dispatch on `type`.
    ///
    /// The three unimplemented types are rejected by name — see the module `[CYRUP-DELTA]`.
    pub fn from_json(doc: &Value) -> Result<Self, ProviderError> {
        let kind = doc.get("type").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "authorized_user" => Ok(AdcCredentials::AuthorizedUser {
                client_id: require_str(doc, "client_id", kind)?,
                client_secret: require_str(doc, "client_secret", kind)?,
                refresh_token: require_str(doc, "refresh_token", kind)?,
                token_uri: doc
                    .get("token_uri")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(GOOGLE_OAUTH2_TOKEN_URI)
                    .to_string(),
            }),
            "service_account" => Ok(AdcCredentials::ServiceAccount {
                client_email: require_str(doc, "client_email", kind)?,
                private_key: require_str(doc, "private_key", kind)?,
                private_key_id: doc
                    .get("private_key_id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                token_uri: doc
                    .get("token_uri")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or(GOOGLE_OAUTH2_TOKEN_URI)
                    .to_string(),
            }),
            "" => Err(adc_error(
                "Google credentials file has no `type` field; expected `authorized_user` or `service_account`",
            )),
            other => Err(adc_error(format!(
                "Unsupported Google credentials type: {other}. cyrup mints Vertex bearers from \
                 `authorized_user` and `service_account` credentials only (see \
                 auth/google_adc.rs's CYRUP-DELTA note)"
            ))),
        }
    }
}

fn require_str(doc: &Value, field: &str, kind: &str) -> Result<String, ProviderError> {
    doc.get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| adc_error(format!("Google `{kind}` credentials file is missing `{field}`")))
}

fn adc_error(message: impl Into<String>) -> ProviderError {
    ProviderError::Transport(message.into().into())
}

// ---------------------------------------------------------------------------
// RS256 assertion
// ---------------------------------------------------------------------------

/// base64url, no padding — the JWS encoding (RFC 7515 §2).
fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Strip the PEM armour from a PKCS#8 private key and base64-decode the body.
///
/// GCP service-account files carry the key as a `\n`-escaped
/// `-----BEGIN PRIVATE KEY-----` block, i.e. unencrypted PKCS#8, which is exactly what
/// [`ring::signature::RsaKeyPair::from_pkcs8`] consumes.
pub fn pkcs8_der_from_pem(pem: &str) -> Result<Vec<u8>, ProviderError> {
    const BEGIN: &str = "-----BEGIN PRIVATE KEY-----";
    const END: &str = "-----END PRIVATE KEY-----";
    let start = pem
        .find(BEGIN)
        .ok_or_else(|| adc_error("Google service-account `private_key` is not a PKCS#8 PEM block"))?
        + BEGIN.len();
    let end = pem[start..]
        .find(END)
        .ok_or_else(|| adc_error("Google service-account `private_key` PEM block is unterminated"))?
        + start;
    let body: String = pem[start..end].chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(body.as_bytes())
        .map_err(|e| adc_error(format!("Google service-account `private_key` is not valid base64: {e}")))
}

/// Build the signed-but-not-yet-exchanged JWT assertion for a service account.
///
/// `now_secs` is injected so the claim set is testable; production passes the wall clock.
pub fn sign_service_account_assertion(
    client_email: &str,
    private_key_pem: &str,
    private_key_id: Option<&str>,
    token_uri: &str,
    now_secs: i64,
) -> Result<String, ProviderError> {
    let header = match private_key_id {
        Some(kid) => serde_json::json!({ "alg": "RS256", "typ": "JWT", "kid": kid }),
        None => serde_json::json!({ "alg": "RS256", "typ": "JWT" }),
    };
    let claims = serde_json::json!({
        "iss": client_email,
        "scope": CLOUD_PLATFORM_SCOPE,
        "aud": token_uri,
        "exp": now_secs + ASSERTION_LIFETIME_SECS,
        "iat": now_secs,
    });

    let signing_input = format!(
        "{}.{}",
        b64url(header.to_string().as_bytes()),
        b64url(claims.to_string().as_bytes())
    );

    let der = pkcs8_der_from_pem(private_key_pem)?;
    let key_pair = ring::signature::RsaKeyPair::from_pkcs8(&der)
        .map_err(|e| adc_error(format!("Google service-account `private_key` was rejected: {e}")))?;
    let mut signature = vec![0u8; key_pair.public().modulus_len()];
    key_pair
        .sign(
            &ring::signature::RSA_PKCS1_SHA256,
            &ring::rand::SystemRandom::new(),
            signing_input.as_bytes(),
            &mut signature,
        )
        .map_err(|_| adc_error("Failed to sign the Google service-account assertion"))?;

    Ok(format!("{signing_input}.{}", b64url(&signature)))
}

// ---------------------------------------------------------------------------
// Token exchange
// ---------------------------------------------------------------------------

/// The subset of Google's token response this module reads. `expires_in` is seconds.
#[derive(Debug, serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
}

/// Wall clock in Unix milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// POST a form to a Google token endpoint and read the access token out of the response.
async fn post_token_form(
    client: &reqwest::Client,
    token_uri: &str,
    form: &[(&str, &str)],
    now_ms: i64,
) -> Result<AdcToken, ProviderError> {
    // `application/x-www-form-urlencoded` by hand: `reqwest`'s `.form()` needs the `urlencoded`
    // feature, which this workspace's reqwest edge does not enable. `encode_query` is the crate's
    // own `URLSearchParams.toString()` port, already used by every OAuth flow.
    let body = crate::auth::oauth::query::encode_query(form.iter().copied());
    let response = client
        .post(token_uri)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(body)
        .send()
        .await
        .map_err(|e| adc_error(format!("Google token request to {token_uri} failed: {e}")))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProviderError::Http {
            status: status.as_u16(),
            message: crate::utils::error_body::normalize_error_body(&body),
        });
    }

    into_token(&body, now_ms)
}

/// Parse a token endpoint's JSON body into an [`AdcToken`]. A response with no `expires_in` is
/// treated as one hour, matching `google-auth-library`'s fallback.
fn into_token(body: &str, now_ms: i64) -> Result<AdcToken, ProviderError> {
    let parsed: TokenResponse = serde_json::from_str(body)
        .map_err(|e| adc_error(format!("Google token response was not valid JSON: {e}")))?;
    if parsed.access_token.is_empty() {
        return Err(adc_error("Google token response carried an empty access_token"));
    }
    let ttl = parsed.expires_in.unwrap_or(ASSERTION_LIFETIME_SECS).max(0);
    Ok(AdcToken {
        access_token: parsed.access_token,
        expires_at_ms: now_ms + ttl * 1000,
    })
}

/// Mint a token for one already-parsed credentials document.
async fn mint_from_credentials(
    creds: &AdcCredentials,
    client: &reqwest::Client,
    now_ms: i64,
) -> Result<AdcToken, ProviderError> {
    match creds {
        AdcCredentials::AuthorizedUser {
            client_id,
            client_secret,
            refresh_token,
            token_uri,
        } => {
            let form = [
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("refresh_token", refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ];
            post_token_form(client, token_uri, &form, now_ms).await
        }
        AdcCredentials::ServiceAccount {
            client_email,
            private_key,
            private_key_id,
            token_uri,
        } => {
            let assertion = sign_service_account_assertion(
                client_email,
                private_key,
                private_key_id.as_deref(),
                token_uri,
                now_ms / 1000,
            )?;
            let form = [
                ("grant_type", JWT_BEARER_GRANT_TYPE),
                ("assertion", assertion.as_str()),
            ];
            post_token_form(client, token_uri, &form, now_ms).await
        }
    }
}

/// Ask the GCE / Cloud Run metadata server for the default service account's token.
async fn mint_from_metadata(
    client: &reqwest::Client,
    now_ms: i64,
) -> Result<AdcToken, ProviderError> {
    let response = client
        .get(GCE_METADATA_TOKEN_URL)
        .header("Metadata-Flavor", "Google")
        .send()
        .await
        .map_err(|e| {
            adc_error(format!(
                "No Google credentials file was found and the GCE metadata server is unreachable: {e}. \
                 Run `gcloud auth application-default login`, or set GOOGLE_APPLICATION_CREDENTIALS."
            ))
        })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(ProviderError::Http {
            status: status.as_u16(),
            message: crate::utils::error_body::normalize_error_body(&body),
        });
    }
    into_token(&body, now_ms)
}

/// Resolve a Vertex bearer token, minting one if no fresh token is cached.
///
/// This is the entry point [`crate::api::google_vertex`] calls on the ADC arm — the arm where
/// `GoogleVertexApiKeyAuth::resolve` returned an [`crate::auth::AuthResult`] with **no** key
/// (pi `google-vertex.ts:76-82`).
pub async fn resolve_access_token(
    ctx: &dyn AuthContext,
    env: Option<&ProviderEnv>,
) -> Result<String, ProviderError> {
    let source = resolve_source(ctx, env).await;
    let key = source.cache_key();
    let now_ms = now_millis();
    if let Some(cached) = cache_get(&key, now_ms) {
        return Ok(cached.access_token);
    }

    // The token endpoint is not known until the credentials file is parsed, and the ported proxy
    // resolver is keyed on the TARGET url — so the file is read and dispatched before the client
    // is built, and is read exactly once.
    let creds = match &source {
        AdcSource::Metadata => None,
        AdcSource::File(path) => {
            let raw = tokio::fs::read_to_string(path).await.map_err(|e| {
                adc_error(format!("Failed to read Google credentials file {path}: {e}"))
            })?;
            let doc: Value = serde_json::from_str(&raw).map_err(|e| {
                adc_error(format!(
                    "Google credentials file {path} is not valid JSON: {e}"
                ))
            })?;
            Some(AdcCredentials::from_json(&doc)?)
        }
    };

    let token_uri = match &creds {
        None => GCE_METADATA_TOKEN_URL,
        Some(
            AdcCredentials::AuthorizedUser { token_uri, .. }
            | AdcCredentials::ServiceAccount { token_uri, .. },
        ) => token_uri.as_str(),
    };

    let client = crate::stream::sse::build_client_for_target(
        token_uri,
        ctx,
        env,
        // The token round trip takes the process-global idle timeout, as every non-streaming
        // request does; a per-request stream timeout must not truncate it.
        None,
    )
    .await?;

    let token = match &creds {
        None => mint_from_metadata(&client, now_ms).await?,
        Some(creds) => mint_from_credentials(creds, &client, now_ms).await?,
    };

    cache_put(&key, &token);
    Ok(token.access_token)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// A throwaway 2048-bit PKCS#8 key. Authenticates nothing; see the file's own header.
    const TEST_KEY_PEM: &str = include_str!("testdata/service_account_test_key.pem");

    struct FakeCtx {
        env: BTreeMap<String, String>,
        files: BTreeSet<String>,
    }

    impl FakeCtx {
        fn new() -> Self {
            Self {
                env: BTreeMap::new(),
                files: BTreeSet::new(),
            }
        }
        fn with_env(mut self, k: &str, v: &str) -> Self {
            self.env.insert(k.to_string(), v.to_string());
            self
        }
        fn with_file(mut self, p: &str) -> Self {
            self.files.insert(p.to_string());
            self
        }
    }

    #[async_trait::async_trait]
    impl AuthContext for FakeCtx {
        async fn env(&self, name: &str) -> Option<String> {
            self.env.get(name).cloned()
        }
        async fn file_exists(&self, path: &str) -> bool {
            self.files.contains(path)
        }
    }

    // ------------------------------------------------------------------ source resolution

    #[tokio::test]
    async fn explicit_credentials_env_wins_and_is_home_expanded() {
        let ctx = FakeCtx::new()
            .with_env("HOME", "/home/dev")
            .with_env(GOOGLE_APPLICATION_CREDENTIALS_ENV, "~/keys/sa.json");
        assert_eq!(
            resolve_source(&ctx, None).await,
            AdcSource::File("/home/dev/keys/sa.json".to_string())
        );
    }

    #[tokio::test]
    async fn scoped_overlay_beats_the_ambient_env() {
        let ctx = FakeCtx::new()
            .with_env("HOME", "/home/dev")
            .with_env(GOOGLE_APPLICATION_CREDENTIALS_ENV, "/ambient.json");
        let mut overlay = ProviderEnv::new();
        overlay.insert(
            GOOGLE_APPLICATION_CREDENTIALS_ENV.to_string(),
            "/scoped.json".to_string(),
        );
        assert_eq!(
            resolve_source(&ctx, Some(&overlay)).await,
            AdcSource::File("/scoped.json".to_string())
        );
    }

    #[tokio::test]
    async fn falls_back_to_the_well_known_gcloud_file() {
        let ctx = FakeCtx::new()
            .with_env("HOME", "/home/dev")
            .with_file("/home/dev/.config/gcloud/application_default_credentials.json");
        assert_eq!(
            resolve_source(&ctx, None).await,
            AdcSource::File(
                "/home/dev/.config/gcloud/application_default_credentials.json".to_string()
            )
        );
    }

    #[tokio::test]
    async fn falls_back_to_the_metadata_server_when_no_file_exists() {
        let ctx = FakeCtx::new().with_env("HOME", "/home/dev");
        assert_eq!(resolve_source(&ctx, None).await, AdcSource::Metadata);
    }

    // ------------------------------------------------------------------ credential dispatch

    #[test]
    fn authorized_user_defaults_the_token_uri() {
        let doc = serde_json::json!({
            "type": "authorized_user",
            "client_id": "cid",
            "client_secret": "secret",
            "refresh_token": "rt",
        });
        match AdcCredentials::from_json(&doc).unwrap() {
            AdcCredentials::AuthorizedUser { token_uri, .. } => {
                assert_eq!(token_uri, GOOGLE_OAUTH2_TOKEN_URI);
            }
            other => panic!("wrong arm: {other:?}"),
        }
    }

    #[test]
    fn external_account_is_rejected_by_name_not_silently_ignored() {
        let doc = serde_json::json!({ "type": "external_account" });
        let err = AdcCredentials::from_json(&doc).unwrap_err().to_string();
        assert!(
            err.contains("external_account"),
            "the error must name the unsupported type, got: {err}"
        );
    }

    #[test]
    fn a_missing_refresh_token_names_the_field() {
        let doc = serde_json::json!({
            "type": "authorized_user",
            "client_id": "cid",
            "client_secret": "secret",
        });
        let err = AdcCredentials::from_json(&doc).unwrap_err().to_string();
        assert!(err.contains("refresh_token"), "got: {err}");
    }

    // ------------------------------------------------------------------ RS256 assertion

    #[test]
    fn service_account_assertion_is_three_rs256_segments_with_the_expected_claims() {
        let jwt = sign_service_account_assertion(
            "sa@example.iam.gserviceaccount.com",
            TEST_KEY_PEM,
            Some("kid-1"),
            GOOGLE_OAUTH2_TOKEN_URI,
            1_700_000_000,
        )
        .expect("sign");

        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "a JWS has exactly three segments");

        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header: Value =
            serde_json::from_slice(&engine.decode(parts[0]).expect("header b64")).unwrap();
        assert_eq!(header["alg"], "RS256");
        assert_eq!(header["typ"], "JWT");
        assert_eq!(header["kid"], "kid-1");

        let claims: Value =
            serde_json::from_slice(&engine.decode(parts[1]).expect("claims b64")).unwrap();
        assert_eq!(claims["iss"], "sa@example.iam.gserviceaccount.com");
        assert_eq!(claims["scope"], CLOUD_PLATFORM_SCOPE);
        assert_eq!(claims["aud"], GOOGLE_OAUTH2_TOKEN_URI);
        assert_eq!(claims["iat"], 1_700_000_000i64);
        assert_eq!(
            claims["exp"],
            1_700_000_000i64 + ASSERTION_LIFETIME_SECS,
            "one-hour assertion lifetime"
        );

        // A 2048-bit RSA signature is 256 bytes.
        assert_eq!(engine.decode(parts[2]).expect("sig b64").len(), 256);
    }

    #[test]
    fn assertion_without_a_key_id_omits_the_kid_header() {
        let jwt = sign_service_account_assertion(
            "sa@example.iam.gserviceaccount.com",
            TEST_KEY_PEM,
            None,
            GOOGLE_OAUTH2_TOKEN_URI,
            1_700_000_000,
        )
        .expect("sign");
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header: Value = serde_json::from_slice(
            &engine
                .decode(jwt.split('.').next().unwrap())
                .expect("header b64"),
        )
        .unwrap();
        assert!(header.get("kid").is_none());
    }

    #[test]
    fn a_non_pem_private_key_is_rejected_with_a_pkcs8_message() {
        let err = sign_service_account_assertion(
            "sa@example.com",
            "not a pem",
            None,
            GOOGLE_OAUTH2_TOKEN_URI,
            0,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("PKCS#8"), "got: {err}");
    }

    #[test]
    fn escaped_newlines_in_a_json_private_key_round_trip() {
        // A GCP key file stores the PEM with literal `\n` escapes; serde_json turns those into
        // real newlines, which is what this parser sees. Prove the whitespace filter also
        // survives the CRLF form a Windows checkout can produce.
        let crlf = TEST_KEY_PEM.replace('\n', "\r\n");
        assert!(pkcs8_der_from_pem(&crlf).is_ok());
    }

    // ------------------------------------------------------------------ token parsing / cache

    #[test]
    fn expires_in_becomes_an_absolute_millisecond_deadline() {
        let token = into_token(r#"{"access_token":"tok","expires_in":3599}"#, 1_000).unwrap();
        assert_eq!(token.access_token, "tok");
        assert_eq!(token.expires_at_ms, 1_000 + 3_599_000);
    }

    #[test]
    fn a_response_with_no_expires_in_defaults_to_one_hour() {
        let token = into_token(r#"{"access_token":"tok"}"#, 0).unwrap();
        assert_eq!(token.expires_at_ms, ASSERTION_LIFETIME_SECS * 1000);
    }

    #[test]
    fn an_empty_access_token_is_an_error_not_an_empty_bearer() {
        assert!(into_token(r#"{"access_token":""}"#, 0).is_err());
    }

    #[test]
    fn the_cache_returns_a_fresh_token_and_withholds_a_stale_one() {
        clear_cache();
        let key = AdcSource::File("/tmp/prov030-cache-probe.json".to_string()).cache_key();
        let token = AdcToken {
            access_token: "ya29.cached".into(),
            expires_at_ms: 10_000_000,
        };
        cache_put(&key, &token);

        // Well outside the eager window ⇒ served from cache, no round trip.
        assert_eq!(
            cache_get(&key, 10_000_000 - 600_000).map(|t| t.access_token),
            Some("ya29.cached".to_string())
        );
        // Inside the eager window ⇒ withheld, so `resolve_access_token` re-mints instead of
        // opening a stream on a bearer that dies mid-turn.
        assert_eq!(cache_get(&key, 10_000_000 - 60_000), None);
        // A different source never sees another source's token.
        assert_eq!(cache_get(&AdcSource::Metadata.cache_key(), 0), None);

        clear_cache();
        assert_eq!(cache_get(&key, 0), None);
    }

    #[test]
    fn a_token_inside_the_eager_refresh_window_is_not_fresh() {
        let token = AdcToken {
            access_token: "tok".into(),
            expires_at_ms: 1_000_000,
        };
        // Four minutes before expiry: inside the 5-minute eager window ⇒ must re-mint.
        assert!(!token.is_fresh_at(1_000_000 - 240_000));
        // Six minutes before expiry: outside the window ⇒ reusable.
        assert!(token.is_fresh_at(1_000_000 - 360_000));
    }
}
