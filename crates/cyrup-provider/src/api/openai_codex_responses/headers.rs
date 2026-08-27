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
/// (`openai-codex-responses.ts:1577-1617`).
///
/// Order is load-bearing: the caller's overlays are applied FIRST and the Codex identity headers
/// last, so `Authorization` / `chatgpt-account-id` / `originator` / `User-Agent` cannot be
/// overridden by `model.headers` or `options.headers` (unlike `openai-responses`, where the overlays
/// come last and do win). A `None` overlay value is pi's `headers.delete(key)`.
///
/// `originator: "pi"` and the `pi (...)` User-Agent are sent verbatim, NOT rebranded: the ChatGPT
/// backend gates on this client identity, which makes it protocol, not branding — the same reason
/// `anthropic-messages` sends `claude-cli/<version>` + `x-app: cli` unchanged. Node's
/// `os.release()` (kernel version) has no `std` equivalent and no C dependency is being added for a
/// User-Agent, so the platform triple is `(<os>; <arch>)`; upstream's own browser branch shortens it
/// further, to `pi (browser)`.
pub(super) fn build_sse_headers(
    model: &Model,
    auth: &AuthResult,
    opts: &StreamOptions,
    account_id: &str,
    token: &str,
    session_id: Option<&str>,
) -> HeaderMap {
    // `new Headers(initHeaders)` where initHeaders is `model.headers` (:1583). cyrup splits pi's
    // single `model.headers` into the catalog map plus the per-credential overlay.
    let mut headers = HeaderMap::new();
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
    // `for (const [key, value] of Object.entries(additionalHeaders || {}))` (:1584-1590).
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            header_set(&mut headers, name, value.clone());
        }
    }

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
    header_set(&mut headers, "originator", Some("pi".to_string()));
    header_set(&mut headers, "User-Agent", Some(codex_user_agent()));

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

/// pi `` `pi (${_os.platform()} ${_os.release()}; ${_os.arch()})` `` (`:1594`).
fn codex_user_agent() -> String {
    format!("pi ({}; {})", std::env::consts::OS, std::env::consts::ARCH)
}
