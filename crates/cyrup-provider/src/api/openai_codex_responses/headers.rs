//! Auth & headers.

use super::{ATOB, JWT_CLAIM_PATH};
use crate::HeaderMap;
use crate::auth::AuthResult;
use crate::model::Model;
use crate::stream::StreamOptions;
use base64::Engine as _;
use serde_json::Value;

/// 1:1 port of pi `extractAccountId` (`openai-codex-responses.ts:1564-1575`): decode the JWT
/// payload and read `payload["https://api.openai.com/auth"].chatgpt_account_id`. Every failure —
/// wrong segment count, undecodable payload, absent or empty claim — collapses to upstream's single
/// error string, which is the whole point of its `try`/`catch`.
pub(super) fn extract_account_id(token: &str) -> Result<String, String> {
    const FAILED: &str = "Failed to extract accountId from token";
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(FAILED.to_string());
    }
    let payload_b64 = parts.get(1).copied().unwrap_or_default();
    let decoded = ATOB.decode(payload_b64).map_err(|_| FAILED.to_string())?;
    let payload: Value = serde_json::from_slice(&decoded).map_err(|_| FAILED.to_string())?;
    let account_id = payload
        .get(JWT_CLAIM_PATH)
        .and_then(|claim| claim.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        // `if (!accountId) throw` — the empty string is falsy in JS.
        .filter(|s| !s.is_empty())
        .ok_or_else(|| FAILED.to_string())?;
    Ok(account_id.to_string())
}

/// `Headers.set` semantics on cyrup's [`HeaderMap`]: HTTP header names are case-insensitive, so a
/// `set` replaces any differently-cased entry rather than adding a second one.
fn header_set(headers: &mut HeaderMap, name: &str, value: Option<String>) {
    let lower = name.to_ascii_lowercase();
    headers.retain(|k, _| k.to_ascii_lowercase() != lower);
    headers.insert(name.to_string(), value);
}

/// 1:1 port of pi `buildBaseCodexHeaders` + `buildSSEHeaders`
/// (`openai-codex-responses.ts:1640-1661` + `:1663-1681` @f1b2e77f5).
///
/// Order is load-bearing, in three steps. (1) The defaults `originator: "pi"` and the `pi (...)`
/// User-Agent are seeded FIRST (`new Headers({ originator: "pi", "User-Agent": getPiUserAgent() })`,
/// `:1646-1647`, pi `0cf65d2bf` / #10429), so (2) the caller's overlays — `model.headers`, the
/// per-credential `auth.auth.headers`, then `options.headers` — can override or delete them, as
/// every other adapter allows. (3) Only `Authorization` and `chatgpt-account-id` are set LAST
/// (`:1658-1659`) and cannot be overridden. A `None` overlay value is pi's `headers.delete(key)`,
/// so a `None` on `originator` or `User-Agent` removes the default.
///
/// `originator: "pi"` and the `pi (...)` User-Agent are sent verbatim, NOT rebranded: the ChatGPT
/// backend gates on this client identity, which makes it protocol, not branding — the same reason
/// `anthropic-messages` sends `claude-cli/<version>` + `x-app: cli` unchanged. The platform tokens
/// are Node's, built by [`crate::utils::user_agent::platform_user_agent`] — the same function the
/// seven `cyrup (…)` adapters use.
pub(super) fn build_sse_headers(
    model: &Model,
    auth: &AuthResult,
    opts: &StreamOptions,
    account_id: &str,
    token: &str,
    session_id: Option<&str>,
) -> HeaderMap {
    // `new Headers({ originator: "pi", "User-Agent": getPiUserAgent() })` (:1646-1647): the
    // defaults go in first so every overlay below can override them.
    let mut headers = HeaderMap::new();
    header_set(&mut headers, "originator", Some("pi".to_string()));
    header_set(&mut headers, "User-Agent", Some(codex_user_agent()));
    // `headers.set(key, value)` for each of `initHeaders`, i.e. `model.headers` (:1648-1650).
    // cyrup splits pi's single `model.headers` into the catalog map plus the per-credential
    // overlay.
    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            header_set(&mut headers, name, value.clone());
        }
    }
    if let Some(overlay) = &auth.auth.headers {
        for (name, value) in overlay {
            header_set(&mut headers, name, value.clone());
        }
    }
    // `for (const [key, value] of Object.entries(additionalHeaders || {}))` (:1651-1657).
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            header_set(&mut headers, name, value.clone());
        }
    }

    // Only these two are set last (:1658-1659), so no overlay can replace the credential.
    header_set(
        &mut headers,
        "Authorization",
        Some(format!("Bearer {token}")),
    );
    header_set(
        &mut headers,
        "chatgpt-account-id",
        Some(account_id.to_string()),
    );

    // `buildSSEHeaders` (:1670-1678).
    header_set(
        &mut headers,
        "OpenAI-Beta",
        Some("responses=experimental".to_string()),
    );
    header_set(
        &mut headers,
        "accept",
        Some("text/event-stream".to_string()),
    );
    header_set(
        &mut headers,
        "content-type",
        Some("application/json".to_string()),
    );

    if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
        header_set(&mut headers, "session-id", Some(sid.to_string()));
        header_set(&mut headers, "x-client-request-id", Some(sid.to_string()));
    }

    headers
}

/// pi `"User-Agent": getPiUserAgent()` (`openai-codex-responses.ts:1647` @f1b2e77f5).
fn codex_user_agent() -> String {
    crate::utils::user_agent::platform_user_agent("pi")
}
