//! URL helpers shared by the `ConverseStream` REST binding and SigV4's canonical request.

/// The `ConverseStream` REST endpoint for `model_id` (the SDK's URI binding:
/// `POST /model/{modelId}/converse-stream`, with `modelId` percent-encoded because inference-profile
/// ARNs contain `:` and `/`).
pub(super) fn converse_stream_url(endpoint: &str, model_id: &str) -> String {
    format!(
        "{}/model/{}/converse-stream",
        endpoint.trim_end_matches('/'),
        uri_encode(model_id, false)
    )
}

/// Percent-encode per AWS SigV4's `UriEncode`: everything outside `A-Za-z0-9-._~` is escaped, with
/// `/` optionally preserved (true for a path, false for a single path segment).
pub(super) fn uri_encode(value: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric()
            || matches!(c, '-' | '.' | '_' | '~')
            || (keep_slash && c == '/')
        {
            out.push(c);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// The `host[:port]` of `url`, and the path (defaulting to `/`).
pub(super) fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    let host = authority.split(':').next()?;
    if host.is_empty() { None } else { Some(host) }
}

pub(super) fn url_authority(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map(|(_, h)| h)
        .unwrap_or(authority);
    if authority.is_empty() {
        None
    } else {
        Some(authority)
    }
}

pub(super) fn url_path(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match rest.find('/') {
        Some(i) => rest
            .get(i..)
            .unwrap_or("/")
            .split(['?', '#'])
            .next()
            .unwrap_or("/"),
        None => "/",
    }
}
