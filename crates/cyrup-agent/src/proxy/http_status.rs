//! HTTP reason phrases + the two-tier proxy failure text (Pi's `response.statusText` stand-in and
//! `streamProxy`'s `!response.ok` branch, proxy.ts:166-177).

use cyrup_provider::ProviderError;
use serde_json::Value;

/// The canonical HTTP reason phrase for `status` — the Rust stand-in for `fetch`'s
/// `response.statusText`, which undici fills from the same IANA registry. Unknown codes yield an
/// empty phrase, exactly as `statusText` is `""` for a response with no reason phrase.
fn status_text(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Entity",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        511 => "Network Authentication Required",
        _ => "",
    }
}

/// AGENT-013 — pi's two-tier proxy failure text (`packages/agent/src/proxy.ts:167-175` @v0.83.0,
/// `:169-177` @v0.84.1):
///
/// ```js
/// let errorMessage = `Proxy error: ${response.status} ${response.statusText}`;
/// try {
///   const errorData = await response.json();
///   if (errorData.error) { errorMessage = `Proxy error: ${errorData.error}`; }
/// } catch { /* Couldn't parse error response */ }
/// throw new Error(errorMessage);
/// ```
///
/// cyrup's transport hands back `ProviderError::Http { status, message }` where `message` IS the
/// (truncated) response body, so the upgrade is a parse of that body rather than a second read.
/// `errorData.error` is checked for JS truthiness, so an empty string keeps the status tier. Every
/// non-HTTP failure keeps the raw `Display` — pi reaches those through its outer `catch`, which
/// surfaces the thrown value's own message.
pub(crate) fn proxy_error_message(e: &ProviderError) -> String {
    match e {
        ProviderError::Http { status, message } => {
            if let Ok(body) = serde_json::from_str::<Value>(message)
                && let Some(err) = body.get("error").and_then(|v| v.as_str())
                && !err.is_empty()
            {
                return format!("Proxy error: {err}");
            }
            format!("Proxy error: {} {}", status, status_text(*status))
        }
        // AGENT-035 — pi's own abort message. `streamProxy` checks the signal by hand at two
        // points and throws a LITERAL: `if (options.signal?.aborted) { throw new Error("Request
        // aborted by user"); }` — once between reads (`packages/agent/src/proxy.ts:186-190`
        // @v0.83.0, `:188-192` @v0.84.1) and once after the read loop drains (`:208-211` /
        // `:210-213`). The outer catch then puts that text in `partial.errorMessage` (`:215-218`),
        // so it is what an aborted proxy turn shows in the transcript. cyrup surfaced
        // `ProviderError::Aborted`'s `Display` — the bare `"aborted"` — instead.
        //
        // [CYRUP-DELTA] pi has a SECOND abort string on this path that cyrup cannot reproduce: an
        // abort that interrupts `fetch`/`reader.read()` mid-await rejects with undici's own
        // `AbortError`, whose message pi passes through unchanged. cyrup's `open_sse` frame stream
        // is itself cancel-aware (`crates/cyrup-provider/src/stream/sse.rs:406-412`, a `biased`
        // select that yields `ProviderError::Aborted` before polling the body), so both of pi's
        // cases collapse onto one value here and are indistinguishable. The string pi's own SOURCE
        // contains is the one ported; the other is a JS-runtime artifact.
        ProviderError::Aborted => "Request aborted by user".to_string(),
        other => other.to_string(),
    }
}
