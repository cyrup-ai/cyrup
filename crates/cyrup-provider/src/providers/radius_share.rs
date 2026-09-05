//! The Radius **artifact** upload behind `/share` — port of pi v0.84.4
//! `packages/coding-agent/src/modes/interactive/session-share.ts` `tryShareViaRadius` (`:91-150`).
//! DRIFT-053.
//!
//! `/share` gained this path at pi `v0.84.3` (`460191cfc`, "feat(coding-agent): include context in
//! Radius session shares"); the file does not exist at `v0.84.1` or `v0.84.2` and is byte-identical
//! at `v0.84.3` and `v0.84.4` (`git -C tmp/pi diff v0.84.3 v0.84.4 --` on that path is empty). It
//! turns `/share` into **Radius first, private gist as the fallback**: a user who signed in to a
//! Radius gateway gets an organization-visibility artifact, and only a user with no radius
//! credential falls through to publishing a GitHub gist.
//!
//! Why this lives in `cyrup-provider` rather than in the TUI that calls it: the gateway origin
//! ([`super::radius::DEFAULT_RADIUS_GATEWAY`]), the credential that authorizes the upload and the
//! HTTP client that resolves `HTTP(S)_PROXY`/`NO_PROXY` the way provider traffic does are all
//! already here, exactly as upstream keeps `DEFAULT_RADIUS_GATEWAY` in `packages/ai` and imports it
//! into the coding-agent (`session-share.ts:6`).
//!
//! The decision half is separated from the transport half on purpose (see
//! [`classify_artifact_response`]): every branch of pi's `:126-140` is a pure function of the
//! response status and body, so the wording of what the user is told is testable without a socket.

use std::time::Duration;

use cyrup_core::{CancelToken, ProviderId};

use crate::ApiId;
use crate::auth::{
    AuthContext, AuthOverrides, AuthResult, CredentialStore, ProviderAuth, auth_credential,
    resolve_provider_auth,
};
use crate::error::{AuthError, ProviderError};
use crate::model::{Modality, Model, ModelCost};

use super::radius::truncate_http_body;

/// `minOAuthValidityMs: 5 * 60_000` (`session-share.ts:96`) — the upload must not begin with a
/// token that expires mid-flight, so the share path asks for five minutes of remaining validity
/// rather than taking whatever is stored.
pub const RADIUS_SHARE_MIN_OAUTH_VALIDITY_MS: i64 = 5 * 60_000;

/// `url.searchParams.set("visibility", "organization")` (`session-share.ts:113`). This is the whole
/// point of the feature: the share lands inside the user's organization, not on a public URL.
pub const RADIUS_SHARE_VISIBILITY: &str = "organization";

/// `url.searchParams.set("title", "Pi session")` (`session-share.ts:114`), rebranded — it is a
/// user-visible label on the artifact the user themself created, in the same class as the trust
/// banner's product name (`crate`-external: `cyrup-tui/src/app/share.rs`). Nothing on the wire
/// keys off it: the gateway echoes it back as the artifact's display title.
pub const RADIUS_SHARE_ARTIFACT_TITLE: &str = "Cyrup session";

/// `"Content-Type": "application/x-ndjson"` (`session-share.ts:119`) — the export is newline
/// delimited JSON, not a JSON document.
pub const RADIUS_SHARE_CONTENT_TYPE: &str = "application/x-ndjson";

/// Upstream sets no explicit timeout (browser `fetch` semantics); this is the same 15 s ceiling
/// [`super::radius`] applies to the gateway config fetch, so a wedged gateway cannot pin the
/// "Uploading to Radius..." loader open forever.
const DEFAULT_UPLOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// What one upload attempt told the user — pi's `:126-140`, where BOTH arms `return true`, i.e.
/// neither one falls back to the gist path.
///
/// Modelled as a domain enum rather than `Result<String, String>` because a non-2xx gateway reply
/// is an ordinary, expected outcome of `/share` (the user is told and the command ends), not a
/// technical failure the caller must propagate; `Result` here would invite a `?` that turned a
/// reported error into a silent one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RadiusShareOutcome {
    /// `showStatus(\`Share URL: ${hyperlink(shareUrl, shareUrl)}\`)` (`:139`) over
    /// `json.artifact.canonical_url`.
    Shared { url: String },
    /// `showError(\`Failed to upload Radius artifact: ${…}\`)` (`:133-136`, `:144-146`). `detail`
    /// is upstream's interpolated tail only; the caller supplies the sentence, so the two
    /// error sites cannot drift apart.
    Failed { detail: String },
}

/// `new URL("/v1/artifacts", DEFAULT_RADIUS_GATEWAY)` plus the two search params
/// (`session-share.ts:112-114`).
///
/// Like [`super::radius`]'s config URL, an absolute path resolves against the gateway's ORIGIN
/// under the WHATWG rules upstream relies on, so any path the configured gateway carries is
/// discarded.
#[must_use]
pub fn artifacts_url(gateway: &str) -> String {
    let origin = match gateway.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split('/').next().unwrap_or(rest);
            format!("{scheme}://{host}")
        }
        // Unreachable after `normalize_radius_gateway_url`, which always supplies a scheme.
        None => gateway.to_string(),
    };
    format!(
        "{origin}/v1/artifacts?visibility={}&title={}",
        percent_encode_query(RADIUS_SHARE_VISIBILITY),
        percent_encode_query(RADIUS_SHARE_ARTIFACT_TITLE)
    )
}

/// `URLSearchParams` serialization for the two values this module sets: space becomes `+` and every
/// byte outside the unreserved set is percent-encoded. Kept minimal deliberately — a general
/// encoder would be dead code, and both values are compile-time constants.
fn percent_encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The pure half of pi's post-`await` tail (`session-share.ts:126-140`):
///
/// ```ts
/// const json = (await response.json().catch(() => null)) as { artifact?: {canonical_url}; error?: string } | null;
/// if (!response.ok || !json?.artifact) {
///     context.showError(`Failed to upload Radius artifact: ${json?.error || response.statusText || response.status}`);
///     return true;
/// }
/// context.showStatus(`Share URL: ${hyperlink(json.artifact.canonical_url, …)}`);
/// ```
///
/// Three upstream subtleties are load-bearing and each has a test:
///
/// * **A 2xx without an `artifact` is still a failure.** `!json?.artifact` is checked
///   independently of `response.ok`, so a gateway that answers `200 {}` reports an error rather
///   than a share URL built from `undefined`.
/// * **An unparseable body is `null`, not a throw** (`.catch(() => null)`), and `null?.error` is
///   `undefined`, so the detail falls through to the status text.
/// * **`||` is JS falsiness**, so an `error` field that is the empty string does NOT win over the
///   status text, and an empty status text falls through to the numeric status.
#[must_use]
pub fn classify_artifact_response(
    status: u16,
    status_text: &str,
    body: &str,
) -> RadiusShareOutcome {
    let json: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let canonical = json
        .as_ref()
        .and_then(|v| v.get("artifact"))
        .filter(|artifact| !artifact.is_null())
        .and_then(|artifact| artifact.get("canonical_url"))
        .and_then(serde_json::Value::as_str);
    let ok = (200..300).contains(&status);
    match canonical {
        Some(url) if ok => RadiusShareOutcome::Shared {
            url: url.to_string(),
        },
        // `json.artifact` present but `canonical_url` absent is `json?.artifact` truthy in JS, so
        // upstream would render `Share URL: undefined`. cyrup cannot render an absent string, and
        // reporting the failure is the only honest option — the one deliberate divergence here.
        _ => {
            let error = json
                .as_ref()
                .and_then(|v| v.get("error"))
                .and_then(serde_json::Value::as_str)
                .filter(|e| !e.is_empty());
            let detail = match error {
                Some(error) => error.to_string(),
                None if !status_text.is_empty() => status_text.to_string(),
                None => status.to_string(),
            };
            RadiusShareOutcome::Failed { detail }
        }
    }
}

/// `POST {gateway}/v1/artifacts?visibility=organization&title=…` with the JSONL export as the body
/// (`session-share.ts:110-149`).
///
/// `cancel` is upstream's `loader.signal`: pi re-checks `loader.signal.aborted` after the send and
/// again after the body read (`:125`, `:130`) and returns without touching the UI, which is what
/// [`ProviderError::Aborted`] means here — the caller must print nothing, because the cancel path
/// has already printed `Share cancelled`.
///
/// A transport error is pi's `catch` (`:141-149`), which reports `error.message` through the SAME
/// sentence as a non-2xx reply, so it comes back as [`RadiusShareOutcome::Failed`] rather than an
/// `Err` — only an abort is `Err`.
pub async fn upload_share_artifact(
    gateway: &str,
    token: &str,
    body: Vec<u8>,
    ctx: &dyn AuthContext,
    cancel: &CancelToken,
) -> Result<RadiusShareOutcome, ProviderError> {
    upload_share_artifact_with_timeout(gateway, token, body, ctx, cancel, DEFAULT_UPLOAD_TIMEOUT)
        .await
}

async fn upload_share_artifact_with_timeout(
    gateway: &str,
    token: &str,
    body: Vec<u8>,
    ctx: &dyn AuthContext,
    cancel: &CancelToken,
    timeout: Duration,
) -> Result<RadiusShareOutcome, ProviderError> {
    let url = artifacts_url(gateway);
    let client = crate::stream::sse::build_client_for_target(&url, ctx, None, None).await?;
    // `"Content-Length": String(body.byteLength)` (`:120`) is explicit upstream because it posts a
    // Node `Buffer`; `reqwest` sets it from a `Vec<u8>` body itself, so setting it again would only
    // risk a duplicate header.
    let request = client
        .post(&url)
        .timeout(timeout)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", RADIUS_SHARE_CONTENT_TYPE)
        .body(body);

    let response = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(ProviderError::Aborted),
        sent = request.send() => match sent {
            Ok(response) => response,
            // pi's `catch` (`:141-149`): the message, not a propagated failure.
            Err(e) => return Ok(RadiusShareOutcome::Failed { detail: e.to_string() }),
        },
    };
    let status = response.status().as_u16();
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or_default()
        .to_string();
    let body_text = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(ProviderError::Aborted),
        read = response.text() => match read {
            Ok(read) => read,
            Err(e) => return Ok(RadiusShareOutcome::Failed { detail: e.to_string() }),
        },
    };
    // pi re-checks `loader.signal.aborted` after the body read too (`:130`).
    if cancel.is_cancelled() {
        return Err(ProviderError::Aborted);
    }
    Ok(classify_artifact_response(
        status,
        &status_text,
        &truncate_http_body(&body_text),
    ))
}

/// `getAuthCredential(await modelRuntime.getAuth("radius", { minOAuthValidityMs: 5 * 60_000 }))`
/// (`session-share.ts:95-98`) — the token that authorizes the upload, or `None` for "no credential
/// stored and none in the environment", which is pi's `return false` and therefore the gist
/// fallback.
///
/// **Why this takes a `ProviderAuth` rather than going through
/// [`crate::Models::get_auth_with`].** pi resolves auth PROVIDER-scoped: `Models.getAuth`'s string
/// overload (`ai/src/models.ts:546-563` @v0.84.4) calls `resolveProviderAuth(provider, …)` with no
/// model at all, and `ApiKeyAuth.resolve` takes none either — *"Resolution is provider-scoped;
/// model-specific endpoint preparation happens after auth has been resolved"*
/// (`ai/src/auth/types.ts:190-193`). cyrup's [`crate::ApiKeyAuth::resolve`] carries an extra
/// `&Model` parameter, and radius's catalog is DYNAMIC — empty until a gateway refresh has run —
/// so there may be no model to hand it. The model built here is therefore a real, correct
/// description of the provider's own endpoint (`pi-messages` at the gateway) rather than a
/// placeholder that could mislead a strategy which reads it. **`[CYRUP-DELTA]` residual:** the
/// `&Model` on `ApiKeyAuth::resolve` is a port divergence in its own right and is what forces this
/// note; radius's api-key strategy is `env_key(…)`, which ignores it entirely.
pub async fn radius_share_token(
    provider_id: &ProviderId,
    auth: &ProviderAuth,
    gateway: &str,
    credentials: &dyn CredentialStore,
    ctx: &dyn AuthContext,
) -> Result<Option<String>, AuthError> {
    let model = provider_auth_model(provider_id, gateway);
    let resolved: Option<AuthResult> = resolve_provider_auth(
        provider_id,
        auth,
        &model,
        credentials,
        ctx,
        AuthOverrides {
            api_key: None,
            env: None,
            min_oauth_validity_ms: Some(RADIUS_SHARE_MIN_OAUTH_VALIDITY_MS),
        },
    )
    .await?;
    Ok(auth_credential(resolved.as_ref()))
}

/// The provider-scoped stand-in for pi's model-free `resolveProviderAuth` (see
/// [`radius_share_token`]). Every field describes the radius gateway truthfully, so a strategy that
/// reads `provider`, `api` or `base_url` sees what it would see for a real gateway model.
fn provider_auth_model(provider_id: &ProviderId, gateway: &str) -> Model {
    Model {
        id: "radius-share".into(),
        name: "Radius session share".to_string(),
        api: ApiId::from(crate::known_api::PI_MESSAGES),
        provider: provider_id.clone(),
        base_url: gateway.to_string(),
        reasoning: false,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 0,
        max_tokens: 0,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::auth::{Credential, EnvAuthContext, InMemoryCredentialStore};
    use crate::providers::radius::{DEFAULT_RADIUS_GATEWAY, radius_auth};

    /// `new URL("/v1/artifacts", gateway)` + the two `searchParams` (`session-share.ts:112-114`),
    /// including the origin rule any configured path is discarded by.
    #[test]
    fn artifacts_url_is_the_gateway_origin_with_both_search_params() {
        assert_eq!(
            artifacts_url(DEFAULT_RADIUS_GATEWAY),
            "https://radius.pi.dev/v1/artifacts?visibility=organization&title=Cyrup+session"
        );
        assert_eq!(
            artifacts_url("https://gw.example.test/some/path"),
            "https://gw.example.test/v1/artifacts?visibility=organization&title=Cyrup+session"
        );
    }

    /// The success arm: 2xx + `artifact.canonical_url` (`:132`, `:138-139`).
    #[test]
    fn a_2xx_with_an_artifact_is_the_share_url() {
        assert_eq!(
            classify_artifact_response(
                201,
                "Created",
                r#"{"artifact":{"canonical_url":"https://radius.pi.dev/a/xyz"}}"#
            ),
            RadiusShareOutcome::Shared {
                url: "https://radius.pi.dev/a/xyz".to_string()
            }
        );
    }

    /// `!response.ok || !json?.artifact` — the SECOND disjunct on its own. A gateway that answers
    /// `200 {}` must report a failure, not a share URL built from `undefined`.
    #[test]
    fn a_2xx_without_an_artifact_is_a_failure() {
        assert_eq!(
            classify_artifact_response(200, "OK", "{}"),
            RadiusShareOutcome::Failed {
                detail: "OK".to_string()
            }
        );
    }

    /// `${json?.error || response.statusText || response.status}` — all three tiers, plus JS
    /// falsiness on the first two.
    #[test]
    fn the_failure_detail_follows_pis_three_tier_fallback() {
        assert_eq!(
            classify_artifact_response(403, "Forbidden", r#"{"error":"not in an organization"}"#),
            RadiusShareOutcome::Failed {
                detail: "not in an organization".to_string()
            }
        );
        // An EMPTY `error` is falsy in JS, so the status text wins.
        assert_eq!(
            classify_artifact_response(500, "Internal Server Error", r#"{"error":""}"#),
            RadiusShareOutcome::Failed {
                detail: "Internal Server Error".to_string()
            }
        );
        // An empty status text falls through to the numeric status.
        assert_eq!(
            classify_artifact_response(599, "", "nope"),
            RadiusShareOutcome::Failed {
                detail: "599".to_string()
            }
        );
    }

    /// `.catch(() => null)`: an unparseable body is `null`, and `null?.error` is `undefined` — it
    /// must not become part of the message and must not panic.
    #[test]
    fn an_unparseable_body_falls_through_to_the_status_text() {
        assert_eq!(
            classify_artifact_response(502, "Bad Gateway", "<html>nginx</html>"),
            RadiusShareOutcome::Failed {
                detail: "Bad Gateway".to_string()
            }
        );
    }

    /// `getAuthCredential(await getAuth("radius", …))` with a stored OAuth credential: the access
    /// token is what authorizes the upload, and it is read through the ordinary request-auth path
    /// so an expiring token is refreshed rather than sent.
    #[tokio::test]
    async fn a_stored_oauth_credential_yields_its_access_token() {
        let id = ProviderId::from("radius");
        let store = InMemoryCredentialStore::new().with_credential(
            id.clone(),
            Credential::Oauth {
                refresh: "r".to_string(),
                access: "at-123".to_string(),
                // Far future: no refresh is attempted, so no socket is opened.
                expires: 32_503_680_000_000,
                ext: serde_json::Map::new(),
            },
        );
        let auth = radius_auth("Radius", DEFAULT_RADIUS_GATEWAY);
        let token = radius_share_token(&id, &auth, DEFAULT_RADIUS_GATEWAY, &store, &EnvAuthContext)
            .await
            .unwrap();
        assert_eq!(token.as_deref(), Some("at-123"));
    }

    /// pi's `if (!token) return false` (`:98`) — the ONLY condition that hands `/share` back to the
    /// gist path once a radius provider exists. An empty credential store with no `RADIUS_API_KEY`
    /// in the environment must produce `None`.
    #[tokio::test]
    async fn no_stored_credential_and_no_env_key_yields_none() {
        struct NoEnv;
        #[async_trait::async_trait]
        impl AuthContext for NoEnv {
            async fn env(&self, _name: &str) -> Option<String> {
                None
            }
            async fn file_exists(&self, _path: &str) -> bool {
                false
            }
        }
        let id = ProviderId::from("radius");
        let store = InMemoryCredentialStore::new();
        let auth = radius_auth("Radius", DEFAULT_RADIUS_GATEWAY);
        let token = radius_share_token(&id, &auth, DEFAULT_RADIUS_GATEWAY, &store, &NoEnv)
            .await
            .unwrap();
        assert_eq!(token, None);
    }
}
