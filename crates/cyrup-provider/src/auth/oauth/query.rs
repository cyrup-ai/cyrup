//! `application/x-www-form-urlencoded` parsing and serialization.
//!
//! pi gets this free from the platform: every flow reads callback parameters through
//! `new URL(req.url, ...).searchParams` (`openrouter.ts:169`, `radius.ts:177`) and builds
//! authorize URLs through `new URLSearchParams({...}).toString()` (`openrouter.ts:257-261`,
//! `github-copilot.ts:214-218`). Rust's std has neither, and this crate has no URL dependency, so
//! the two halves of `URLSearchParams` are implemented here to the WHATWG URL spec §6 rules that
//! pi's callers depend on:
//!
//! * parse: split on `&`, then `=`; `+` decodes to space; `%XX` is percent-decoded; a key with no
//!   `=` yields an empty value.
//! * serialize: space encodes to `+`; only `*`, `-`, `.`, `_` and ASCII alphanumerics survive
//!   unescaped — everything else becomes uppercase `%XX` over its UTF-8 bytes.

/// Percent/plus-decode one `application/x-www-form-urlencoded` component.
///
/// Invalid `%` escapes are passed through literally, matching the WHATWG percent-decode
/// algorithm (and therefore `URLSearchParams`), which never throws.
fn decode_component(raw: &str) -> String {
    decode(raw, true)
}

/// Percent-decode without the `+`→space rule — URL *path* semantics, which
/// [`super::callback`] needs for the pathname it compares against the configured callback path
/// (`new URL(req.url, base).pathname`).
pub fn percent_decode(raw: &str) -> String {
    decode(raw, false)
}

fn decode(raw: &str, plus_as_space: bool) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while let Some(byte) = bytes.get(i).copied() {
        match byte {
            b'+' if plus_as_space => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hi = bytes.get(i + 1).copied().and_then(hex_val);
                let lo = bytes.get(i + 2).copied().and_then(hex_val);
                match (hi, lo) {
                    (Some(hi), Some(lo)) => {
                        out.push((hi << 4) | lo);
                        i += 3;
                    }
                    _ => {
                        out.push(byte);
                        i += 1;
                    }
                }
            }
            _ => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Parse a query string (with or without a leading `?`) into ordered key/value pairs, the way
/// `URLSearchParams` does. Duplicate keys are preserved in order; `searchParams.get(name)` is
/// "the first one", which is what [`super::CallbackRequest::param`] implements.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    let query = query.strip_prefix('?').unwrap_or(query);
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (decode_component(k), decode_component(v)),
            None => (decode_component(pair), String::new()),
        })
        .collect()
}

/// Percent-encode one component using the `application/x-www-form-urlencoded` serializer set.
fn encode_component(value: &str, out: &mut String) {
    for byte in value.as_bytes() {
        match *byte {
            b' ' => out.push('+'),
            b'*' | b'-' | b'.' | b'_' => out.push(*byte as char),
            b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' => out.push(*byte as char),
            other => {
                out.push('%');
                out.push(
                    char::from_digit((other >> 4) as u32, 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((other & 0x0f) as u32, 16)
                        .unwrap_or('0')
                        .to_ascii_uppercase(),
                );
            }
        }
    }
}

/// Serialize pairs exactly as `new URLSearchParams({...}).toString()` does — the string flows
/// paste into an authorize URL's `search` or a token request's body.
pub fn encode_query<'a, I>(pairs: I) -> String
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut out = String::new();
    for (key, value) in pairs {
        if !out.is_empty() {
            out.push('&');
        }
        encode_component(key, &mut out);
        out.push('=');
        encode_component(value, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    #[test]
    fn parses_a_real_authorization_callback() {
        // The shape an authorization server actually redirects with.
        let pairs = parse_query("?code=abc123&state=xyz&scope=org%3Acreate_api_key+user%3Aprofile");
        assert_eq!(
            pairs,
            vec![
                ("code".to_string(), "abc123".to_string()),
                ("state".to_string(), "xyz".to_string()),
                (
                    "scope".to_string(),
                    "org:create_api_key user:profile".to_string()
                ),
            ]
        );
    }

    #[test]
    fn decodes_plus_as_space_and_percent_escapes() {
        let pairs = parse_query("error=access_denied&error_description=User+cancelled%20login%21");
        assert_eq!(pairs[1].1, "User cancelled login!");
    }

    #[test]
    fn tolerates_malformed_input_like_urlsearchparams() {
        assert_eq!(parse_query(""), Vec::new());
        assert_eq!(parse_query("&&"), Vec::new());
        // A bare key gets an empty value; a truncated escape is passed through literally.
        assert_eq!(
            parse_query("code&state=%2"),
            vec![
                ("code".to_string(), String::new()),
                ("state".to_string(), "%2".to_string()),
            ]
        );
        // `=` inside the value is part of the value (split_once).
        assert_eq!(parse_query("code=a=b")[0].1, "a=b");
    }

    #[test]
    fn duplicate_keys_keep_document_order() {
        let pairs = parse_query("code=first&code=second");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].1, "first");
        assert_eq!(pairs[1].1, "second");
    }

    /// The exact string `new URLSearchParams(...).toString()` produces for pi's Anthropic
    /// authorize params (`anthropic.ts` SCOPES is a space-separated list, which the urlencoded
    /// serializer turns into `+`).
    #[test]
    fn serializes_like_url_search_params() {
        let encoded = encode_query([
            ("code_challenge_method", "S256"),
            ("scope", "org:create_api_key user:profile"),
            ("redirect_uri", "http://localhost:53692/callback"),
        ]);
        assert_eq!(
            encoded,
            "code_challenge_method=S256\
             &scope=org%3Acreate_api_key+user%3Aprofile\
             &redirect_uri=http%3A%2F%2Flocalhost%3A53692%2Fcallback"
        );
    }

    #[test]
    fn serializer_keeps_the_urlencoded_unreserved_set() {
        assert_eq!(encode_query([("a", "*-._~")]), "a=*-._%7E");
        assert_eq!(encode_query([("a", "é")]), "a=%C3%A9");
        assert_eq!(encode_query(Vec::<(&str, &str)>::new()), "");
    }

    /// Path decoding keeps `+` literal — only the urlencoded serializer means space by it.
    #[test]
    fn percent_decode_leaves_plus_alone() {
        assert_eq!(percent_decode("/oauth/call+back"), "/oauth/call+back");
        assert_eq!(percent_decode("/oauth/%63allback"), "/oauth/callback");
        assert_eq!(decode_component("a+b"), "a b");
    }

    /// Round-trip: whatever the serializer emits, the parser recovers.
    #[test]
    fn round_trips() {
        let value = "a b+c&d=e%f/ü";
        let encoded = encode_query([("k", value)]);
        assert_eq!(
            parse_query(&encoded),
            vec![("k".to_string(), value.to_string())]
        );
    }
}
