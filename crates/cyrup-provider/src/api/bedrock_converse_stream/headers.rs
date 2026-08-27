//! Headers (pi `addCustomHeadersMiddleware`, `bedrock-converse-stream.ts:373-401`).

use super::config::BedrockClientConfig;
use super::sigv4::{now_unix_seconds, sign_sigv4};
use crate::HeaderMap;
use std::collections::BTreeMap;

/// pi `RESERVED_HEADER_EXACT` (`bedrock-converse-stream.ts:373`).
const RESERVED_HEADER_EXACT: [&str; 2] = ["authorization", "host"];

/// pi `isReservedHeader` (`bedrock-converse-stream.ts:375-378`): case-insensitive, and every
/// `x-amz-*` key is reserved because it participates in the SigV4 canonical request.
pub(super) fn is_reserved_header(key: &str) -> bool {
    let lower = key.to_lowercase();
    lower.starts_with("x-amz-") || RESERVED_HEADER_EXACT.contains(&lower.as_str())
}

/// pi `providerHeadersToRecord` + `addCustomHeadersMiddleware` (`headers.ts:10-17`,
/// `bedrock-converse-stream.ts:387-401`), collapsed: a `None` value drops the entry (pi's
/// `value !== null` filter), reserved keys are skipped, and every other caller header **overrides**
/// any same-named header already on the request. Keys are lower-cased so a mixed-case reserved key
/// cannot slip back in as a distinct header (pi's VC2 case).
///
/// `model.headers` sits below `opts.headers`, matching cyrup's documented overlay order
/// (auth < `model.headers` < `opts.headers`).
pub(super) fn apply_custom_headers(
    headers: &mut BTreeMap<String, String>,
    request_headers: Option<&HeaderMap>,
    model_headers: Option<&HeaderMap>,
) {
    for source in [model_headers, request_headers].into_iter().flatten() {
        for (key, value) in source {
            if is_reserved_header(key) {
                continue;
            }
            match value {
                Some(v) => {
                    headers.insert(key.to_lowercase(), v.clone());
                }
                None => {
                    headers.remove(&key.to_lowercase());
                }
            }
        }
    }
}

/// Install the `Authorization` header: `Bearer <token>` when a bearer token is configured (pi
/// `config.token` + `authSchemePreference: ["httpBearerAuth"]`, `:217-220`), otherwise SigV4.
pub(super) fn authorize(
    headers: &mut BTreeMap<String, String>,
    config: &BedrockClientConfig,
    url: &str,
    body: &[u8],
) -> Result<(), String> {
    if let Some(token) = &config.bearer_token {
        headers.insert("authorization".to_string(), format!("Bearer {token}"));
        return Ok(());
    }
    let Some(creds) = &config.credentials else {
        // The SDK would raise `CredentialsProviderError` here. Surface the same category of
        // failure rather than sending an unsigned request that Bedrock answers with an opaque 403.
        return Err(format!(
            "Could not load credentials from any providers{}",
            config
                .profile
                .as_deref()
                .map(|p| format!(" (profile \"{p}\")"))
                .unwrap_or_default()
        ));
    };
    let region = config.region.clone().unwrap_or_else(|| "us-east-1".into());
    sign_sigv4(headers, url, body, creds, &region, now_unix_seconds())
}
