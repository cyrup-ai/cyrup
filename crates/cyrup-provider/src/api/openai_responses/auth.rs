//! Request encoding: the client API key (Pi `getClientApiKey`, openai-responses.ts:37-41).

use super::headers::header_present;
use crate::auth::AuthResult;
use crate::stream::StreamOptions;

/// Pi `getClientApiKey` (openai-responses.ts:37-41) + the WireProvider-resolved key. A resolved key
/// wins; otherwise an `authorization`/`cf-aig-authorization` header (from the auth or opts overlay)
/// lets the SDK send the literal `"unused"`; otherwise `None` (caller errors).
pub(super) fn resolve_api_key(auth: &AuthResult, opts: &StreamOptions) -> Option<String> {
    if let Some(key) = &auth.auth.api_key {
        return Some(key.clone());
    }
    let has = |name: &str| {
        header_present(auth.auth.headers.as_ref(), name)
            || header_present(opts.headers.as_ref(), name)
    };
    if has("authorization") || has("cf-aig-authorization") {
        return Some("unused".to_string());
    }
    None
}
