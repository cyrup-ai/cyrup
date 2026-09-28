//! HTTP(S) proxy resolution for a target URL (1:1 port of Pi `utils/node-http-proxy.ts`).
//!
//! Honors the standard `*_proxy` / `all_proxy` env vars (lower- and upper-case) with the
//! provider-scoped [`ProviderEnv`] overlay winning over the ambient context, applies `no_proxy`
//! matching (`*`; a bare host, which also exempts its subdomains; `host:port`; a `*.`/`.`/`*`
//! prefix, which exempts the apex and its subdomains; and a bracketed or bare IPv6 host, compared
//! bracket-free — node-http-proxy.ts:37-113 @v0.87.1), fills default ports per scheme, and
//! rejects non-HTTP(S) (SOCKS/PAC) proxy URLs — exactly as Pi's resolver does
//! (`resolveHttpProxyUrlForTarget`, node-http-proxy.ts:92).

use crate::auth::types::{AuthContext, ProviderEnv};

/// Default proxy ports per scheme (Pi `DEFAULT_PROXY_PORTS`, node-http-proxy.ts:4-11).
fn default_proxy_port(scheme: &str) -> u16 {
    match scheme {
        "ftp" => 21,
        "gopher" => 70,
        "http" => 80,
        "https" => 443,
        "ws" => 80,
        "wss" => 443,
        _ => 0,
    }
}

/// Pi's `UNSUPPORTED_PROXY_PROTOCOL_MESSAGE` (node-http-proxy.ts:89).
pub const UNSUPPORTED_PROXY_PROTOCOL_MESSAGE: &str = "Unsupported proxy protocol. SOCKS and PAC proxy URLs are not supported; use an HTTP or HTTPS proxy URL.";

/// A proxy-resolution failure (Pi throws an `Error` for these, node-http-proxy.ts:102-108).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProxyError {
    #[error("Invalid proxy URL {url:?}: {message}")]
    InvalidProxyUrl { url: String, message: String },
    #[error("{UNSUPPORTED_PROXY_PROTOCOL_MESSAGE} Got {protocol}")]
    UnsupportedProtocol { protocol: String },
}

/// `getProxyEnv(key, env)` (node-http-proxy.ts:13-23): lower-case overlay, upper-case overlay,
/// ambient lower-case, ambient upper-case — first non-empty wins (`||` skips empty strings).
async fn get_proxy_env(key: &str, ctx: &dyn AuthContext, env: Option<&ProviderEnv>) -> String {
    let lower = key.to_lowercase();
    let upper = key.to_uppercase();
    let from_overlay = |name: &str| -> Option<String> {
        env.and_then(|e| e.get(name))
            .filter(|v| !v.is_empty())
            .cloned()
    };
    if let Some(v) = from_overlay(&lower) {
        return v;
    }
    if let Some(v) = from_overlay(&upper) {
        return v;
    }
    if let Some(v) = ctx.env(&lower).await.filter(|v| !v.is_empty()) {
        return v;
    }
    if let Some(v) = ctx.env(&upper).await.filter(|v| !v.is_empty()) {
        return v;
    }
    // PROV-047. pi's `applyHttpProxySettings` (`coding-agent/src/core/http-dispatcher.ts:43-48`
    // @v0.83.0) writes the `httpProxy` SETTING into the process environment as
    // `process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`, so by the time
    // `getProxyEnv` runs it is indistinguishable from an ambient variable — which is precisely why
    // the setting reaches every egress path upstream and reached exactly one here.
    //
    // cyrup cannot write its own environment (see `sse::configure_http_proxy`), so the setting is
    // consulted at this layer instead, and ONLY for the two names pi writes. `??=` gives the
    // ambient variable precedence, which the four lookups above already have.
    //
    // One corner is not reproduced and is stated rather than hidden: pi's `??=` does not overwrite
    // an ambient variable explicitly set to the EMPTY string, and `getProxyEnv`'s `||` then skips
    // it, so `HTTPS_PROXY=""` upstream means "no proxy" even with the setting configured. Here the
    // empty value is indistinguishable from unset and the setting applies.
    if (lower == "http_proxy" || lower == "https_proxy")
        && let Some(configured) = crate::stream::sse::configured_http_proxy()
    {
        return configured;
    }
    String::new()
}

/// `stripBrackets(host)` (node-http-proxy.ts:37-39 @v0.87.1): unwrap a bracketed IPv6 literal.
fn strip_brackets(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
}

/// `parseNoProxyEntry(entry)` (node-http-proxy.ts:41-72 @v0.87.1): trim, lower-case, then one of
/// three shapes — a bracketed `[host]` with an optional `:port`; a bare string with more than one
/// `:`, which is a portless IPv6 host; otherwise a single trailing `:port` split. `port == 0` means
/// "no port qualifier", matching upstream's `Number.isNaN → 0` and its falsy-`0` guard later.
fn parse_no_proxy_entry(entry: &str) -> Option<(String, u16)> {
    let trimmed = entry.trim().to_lowercase();
    if trimmed.is_empty() {
        return None;
    }

    // `if (trimmed.startsWith("["))` (:45-56).
    if trimmed.starts_with('[')
        && let Some(closing) = trimmed.find(']')
    {
        let host = trimmed[1..closing].to_string();
        let rest = &trimmed[closing + 1..];
        return match rest.strip_prefix(':') {
            // `Number.parseInt` on a non-numeric rest is NaN → 0.
            Some(p) => Some((host, p.parse::<u16>().unwrap_or(0))),
            None => Some((host, 0)),
        };
    }

    // `if (trimmed.includes(":") && trimmed.split(":").length > 2)` (:58-60): a bare IPv6 host such
    // as `::1` has no port, and must NOT be split on its last colon.
    if trimmed.matches(':').count() > 1 {
        return Some((trimmed, 0));
    }

    // `colonIndex === trimmed.indexOf(":")` (:62-69): exactly one colon, so split a trailing port.
    if let Some((host, port)) = trimmed.split_once(':')
        && let Ok(port) = port.parse::<u16>()
    {
        return Some((host.to_string(), port));
    }

    Some((trimmed, 0))
}

/// `shouldProxyHostname(hostname, port, env)` (node-http-proxy.ts:74-113 @v0.87.1): consult
/// `no_proxy`. An entry exempts the target when the normalised host EQUALS the entry's domain or is
/// a subdomain of it (`endsWith(".{domain}")`, `:106-112`) — a bare `example.com` entry therefore
/// covers `api.example.com` too, and `*.`/`.`/`*` prefixes are stripped so the apex is covered as
/// well.
///
/// `[CYRUP-DELTA]` — upstream's `hostname.toLowerCase()` (`:82`) is near-inert here because
/// `reqwest::Url::parse` already lower-cases a domain host; it is ported for fidelity, not effect.
async fn should_proxy_hostname(
    hostname: &str,
    port: u16,
    ctx: &dyn AuthContext,
    env: Option<&ProviderEnv>,
) -> bool {
    let no_proxy = get_proxy_env("no_proxy", ctx, env).await.to_lowercase();
    if no_proxy.is_empty() {
        return true;
    }
    if no_proxy == "*" {
        return false;
    }
    let lowered = hostname.to_lowercase();
    let normalized_target = strip_brackets(&lowered);
    // `.every(...)`: proxy iff EVERY no_proxy entry permits it.
    no_proxy
        .split(|c: char| c == ',' || c.is_whitespace())
        .all(|entry| {
            let Some((entry_host, entry_port)) = parse_no_proxy_entry(entry) else {
                return true;
            };
            // A port-qualified entry that targets a different port never blocks (`:91-93`).
            if entry_port != 0 && entry_port != port {
                return true;
            }
            // `*.` is TWO chars and must be stripped before the one-char `.`/`*` cases (`:96-100`);
            // stripping only `*` would leave `.star.net` and lose the apex `star.net`.
            let domain = strip_brackets(&entry_host);
            let domain = match domain.strip_prefix("*.") {
                Some(rest) => rest,
                None => domain
                    .strip_prefix('.')
                    .unwrap_or_else(|| domain.strip_prefix('*').unwrap_or(domain)),
            };
            if domain.is_empty() {
                return true;
            }
            normalized_target != domain && !normalized_target.ends_with(&format!(".{domain}"))
        })
}

/// `getProxyForUrl(targetUrl, env)` (node-http-proxy.ts:69-87): the raw proxy string for a target,
/// or empty when no proxy applies.
async fn get_proxy_for_url(
    target_url: &str,
    ctx: &dyn AuthContext,
    env: Option<&ProviderEnv>,
) -> String {
    let Ok(parsed) = reqwest::Url::parse(target_url) else {
        return String::new();
    };
    let scheme = parsed.scheme();
    let Some(hostname) = parsed.host_str() else {
        return String::new();
    };
    // `stripBrackets(parsedUrl.hostname ...)` (:119): an IPv6 target and an IPv6 no_proxy entry
    // must compare in the same bracket-free form.
    let hostname = strip_brackets(hostname);
    if scheme.is_empty() || hostname.is_empty() {
        return String::new();
    }
    let port = parsed.port().unwrap_or_else(|| default_proxy_port(scheme));
    if !should_proxy_hostname(hostname, port, ctx, env).await {
        return String::new();
    }
    let mut proxy = get_proxy_env(&format!("{scheme}_proxy"), ctx, env).await;
    if proxy.is_empty() {
        proxy = get_proxy_env("all_proxy", ctx, env).await;
    }
    // `if (proxy && !proxy.includes("://")) proxy = `${protocol}://${proxy}``.
    if !proxy.is_empty() && !proxy.contains("://") {
        proxy = format!("{scheme}://{proxy}");
    }
    proxy
}

/// Resolve the HTTP(S) proxy URL to use for `target_url`, if any (Pi
/// `resolveHttpProxyUrlForTarget`, node-http-proxy.ts:92-112). `Ok(None)` when no proxy applies;
/// `Err` for an unparseable proxy URL or a non-HTTP(S) (SOCKS/PAC) proxy scheme.
pub async fn resolve_http_proxy_url_for_target(
    target_url: &str,
    ctx: &dyn AuthContext,
    env: Option<&ProviderEnv>,
) -> Result<Option<reqwest::Url>, ProxyError> {
    let proxy = get_proxy_for_url(target_url, ctx, env).await;
    if proxy.is_empty() {
        return Ok(None);
    }
    let proxy_url = reqwest::Url::parse(&proxy).map_err(|e| ProxyError::InvalidProxyUrl {
        url: proxy.clone(),
        message: e.to_string(),
    })?;
    let scheme = proxy_url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(ProxyError::UnsupportedProtocol {
            protocol: format!("{scheme}:"),
        });
    }
    Ok(Some(proxy_url))
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
    use std::collections::BTreeMap;

    struct MapEnv(BTreeMap<String, String>);
    #[async_trait::async_trait]
    impl AuthContext for MapEnv {
        async fn env(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
        async fn file_exists(&self, _path: &str) -> bool {
            false
        }
    }
    fn ctx<const N: usize>(pairs: [(&str, &str); N]) -> MapEnv {
        MapEnv(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[tokio::test]
    async fn no_proxy_env_yields_none() {
        let env = ctx([]);
        let out = resolve_http_proxy_url_for_target("https://api.example.com/v1", &env, None)
            .await
            .expect("ok");
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn https_proxy_env_resolves_for_https_target() {
        let env = ctx([("https_proxy", "http://proxy.local:8080")]);
        let out = resolve_http_proxy_url_for_target("https://api.example.com/v1", &env, None)
            .await
            .expect("ok")
            .expect("proxy");
        assert_eq!(out.as_str(), "http://proxy.local:8080/");
    }

    #[tokio::test]
    async fn bare_proxy_value_gets_scheme_prefix() {
        // No `://` → prefixed with the target scheme (Pi node-http-proxy.ts:83-85).
        let env = ctx([("http_proxy", "proxy.local:3128")]);
        let out = resolve_http_proxy_url_for_target("http://api.example.com/", &env, None)
            .await
            .expect("ok")
            .expect("proxy");
        assert_eq!(out.scheme(), "http");
        assert_eq!(out.host_str(), Some("proxy.local"));
        assert_eq!(out.port(), Some(3128));
    }

    #[tokio::test]
    async fn overlay_env_wins_over_ambient() {
        let env = ctx([("https_proxy", "http://ambient:1")]);
        let overlay: ProviderEnv = [("https_proxy".to_string(), "http://overlay:2".to_string())]
            .into_iter()
            .collect();
        let out = resolve_http_proxy_url_for_target("https://x.example.com/", &env, Some(&overlay))
            .await
            .expect("ok")
            .expect("proxy");
        assert_eq!(out.host_str(), Some("overlay"));
    }

    #[tokio::test]
    async fn no_proxy_star_disables_all() {
        let env = ctx([("https_proxy", "http://proxy:8080"), ("no_proxy", "*")]);
        let out = resolve_http_proxy_url_for_target("https://api.example.com/", &env, None)
            .await
            .expect("ok");
        assert!(out.is_none());
    }

    #[tokio::test]
    async fn no_proxy_suffix_and_exact_match() {
        // Suffix match (`.example.com`) excludes the host; an unrelated host still proxies.
        let env = ctx([
            ("https_proxy", "http://proxy:8080"),
            ("no_proxy", ".example.com"),
        ]);
        assert!(
            resolve_http_proxy_url_for_target("https://api.example.com/", &env, None)
                .await
                .expect("ok")
                .is_none()
        );
        assert!(
            resolve_http_proxy_url_for_target("https://other.test/", &env, None)
                .await
                .expect("ok")
                .is_some()
        );
    }

    #[tokio::test]
    async fn no_proxy_port_qualified_only_blocks_matching_port() {
        // `host:443` blocks the default-port https target but not an explicit :8443 one.
        let env = ctx([
            ("https_proxy", "http://proxy:8080"),
            ("no_proxy", "api.example.com:443"),
        ]);
        assert!(
            resolve_http_proxy_url_for_target("https://api.example.com/", &env, None)
                .await
                .expect("ok")
                .is_none()
        );
        assert!(
            resolve_http_proxy_url_for_target("https://api.example.com:8443/", &env, None)
                .await
                .expect("ok")
                .is_some()
        );
    }

    /// Translation of upstream `packages/ai/test/node-http-proxy.test.ts:77-103` @v0.87.1,
    /// "handles subdomain wildcards, IPv6, and ports in NO_PROXY".
    #[tokio::test]
    async fn no_proxy_matches_subdomains_wildcards_ipv6_and_ports() {
        let env = ctx([
            ("https_proxy", "http://proxy:8080"),
            (
                "no_proxy",
                "example.com, .wildcard.org, *.star.net, ::1, [2001:db8::1], 127.0.0.1:8080",
            ),
        ]);
        for target in [
            // A BARE entry exempts the apex AND its subdomains (`:106-112`).
            "https://example.com",
            "https://api.example.com",
            // A leading `.` exempts the apex too, not just subdomains.
            "https://wildcard.org",
            "https://api.wildcard.org",
            // `*.` is stripped as two chars, so the apex is exempt here as well.
            "https://star.net",
            "https://api.star.net",
            // A bare IPv6 entry is portless (`:58-60`) and compares bracket-free (`:82`, `:119`).
            "https://[::1]:80",
            "https://[2001:db8::1]",
            // A port-qualified entry blocks only its own port.
            "https://127.0.0.1:8080",
        ] {
            assert!(
                resolve_http_proxy_url_for_target(target, &env, None)
                    .await
                    .expect("ok")
                    .is_none(),
                "{target} should be exempt"
            );
        }
        for target in [
            // The guard that the subdomain test is `endsWith(".{domain}")` and not a bare
            // `ends_with(domain)`: `notexample.com` is NOT under `example.com`.
            "https://notexample.com",
            "https://127.0.0.1:3000",
        ] {
            assert!(
                resolve_http_proxy_url_for_target(target, &env, None)
                    .await
                    .expect("ok")
                    .is_some(),
                "{target} should be proxied"
            );
        }
    }

    #[tokio::test]
    async fn no_proxy_normalises_target_case() {
        // Pins upstream's `hostname.toLowerCase()` (`:82`) on an APEX target, which the exact-host
        // comparison already handled: this passes at HEAD too, because `reqwest::Url::parse`
        // lower-cases a domain host before the matcher ever sees it. It is here to keep that
        // invariant asserted (see the CYRUP-DELTA on `should_proxy_hostname`), NOT as proof of the
        // subdomain fix above — `no_proxy_matches_subdomains_wildcards_ipv6_and_ports` is that
        // proof. A subdomain target such as `API.Internal.Corp` would also fail at HEAD, but for
        // the subdomain rule rather than for case, so it is deliberately not used here.
        let env = ctx([
            ("https_proxy", "http://proxy:8080"),
            ("no_proxy", "internal.corp"),
        ]);
        assert!(
            resolve_http_proxy_url_for_target("https://INTERNAL.CORP/", &env, None)
                .await
                .expect("ok")
                .is_none()
        );
    }

    #[tokio::test]
    async fn socks_proxy_is_rejected() {
        let env = ctx([("https_proxy", "socks5://proxy:1080")]);
        let err = resolve_http_proxy_url_for_target("https://api.example.com/", &env, None)
            .await
            .expect_err("socks unsupported");
        assert!(matches!(err, ProxyError::UnsupportedProtocol { .. }));
        assert!(err.to_string().contains("SOCKS and PAC"));
    }

    #[tokio::test]
    async fn all_proxy_is_fallback_for_scheme_specific() {
        let env = ctx([("all_proxy", "http://fallback:9")]);
        let out = resolve_http_proxy_url_for_target("https://api.example.com/", &env, None)
            .await
            .expect("ok")
            .expect("proxy");
        assert_eq!(out.host_str(), Some("fallback"));
    }

    // PROV-047's `the_http_proxy_setting_reaches_the_resolver_and_yields_to_everything_above_it`
    // test used to live here. It mutates `crate::stream::sse`'s process-global
    // `HTTP_PROXY_SETTING`, which makes it a hazard to every OTHER concurrently-running test in
    // this crate's shared unit-test binary that builds a real client without its own explicit
    // `http_proxy`/`https_proxy` override (most of this crate's loopback-mock tests) — the same
    // class of bug documented in `crates/cyrup-provider/tests/oauth_http_proxy.rs`'s module doc
    // comment, which is where this test was moved to (a separate `cargo test` OS process, so the
    // global mutation cannot leak into this binary at all). See that file for the full test.
}
