//! HTTP(S) proxy resolution for a target URL (1:1 port of Pi `utils/node-http-proxy.ts`).
//!
//! Honors the standard `*_proxy` / `all_proxy` env vars (lower- and upper-case) with the
//! provider-scoped [`ProviderEnv`] overlay winning over the ambient context, applies `no_proxy`
//! matching (`*`, host, `host:port`, leading-`.`/`*` suffix), fills default ports per scheme, and
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
pub const UNSUPPORTED_PROXY_PROTOCOL_MESSAGE: &str =
    "Unsupported proxy protocol. SOCKS and PAC proxy URLs are not supported; use an HTTP or HTTPS proxy URL.";

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
async fn get_proxy_env(
    key: &str,
    ctx: &dyn AuthContext,
    env: Option<&ProviderEnv>,
) -> String {
    let lower = key.to_lowercase();
    let upper = key.to_uppercase();
    let from_overlay = |name: &str| -> Option<String> {
        env.and_then(|e| e.get(name)).filter(|v| !v.is_empty()).cloned()
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
    String::new()
}

/// `shouldProxyHostname(hostname, port, env)` (node-http-proxy.ts:37-67): consult `no_proxy`.
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
    // `.every(...)`: proxy iff EVERY no_proxy entry permits it.
    no_proxy.split(|c: char| c == ',' || c.is_whitespace()).all(|proxy| {
        if proxy.is_empty() {
            return true;
        }
        // `^(.+):(\d+)$` — split a trailing `:port`.
        let (mut proxy_hostname, proxy_port) = match proxy.rsplit_once(':') {
            Some((host, p)) if !host.is_empty() && !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
                (host, p.parse::<u16>().unwrap_or(0))
            }
            _ => (proxy, 0u16),
        };
        // A port-qualified entry that targets a different port never blocks (returns true → proxy).
        if proxy_port != 0 && proxy_port != port {
            return true;
        }
        // `^[.*]` — entries NOT starting with `.` or `*` are exact-host matches.
        if !proxy_hostname.starts_with('.') && !proxy_hostname.starts_with('*') {
            return hostname != proxy_hostname;
        }
        if let Some(stripped) = proxy_hostname.strip_prefix('*') {
            proxy_hostname = stripped;
        }
        !hostname.ends_with(proxy_hostname)
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
        return Err(ProxyError::UnsupportedProtocol { protocol: format!("{scheme}:") });
    }
    Ok(Some(proxy_url))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
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
        MapEnv(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
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
        let overlay: ProviderEnv =
            [("https_proxy".to_string(), "http://overlay:2".to_string())].into_iter().collect();
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
        let env = ctx([("https_proxy", "http://proxy:8080"), ("no_proxy", ".example.com")]);
        assert!(resolve_http_proxy_url_for_target("https://api.example.com/", &env, None)
            .await
            .expect("ok")
            .is_none());
        assert!(resolve_http_proxy_url_for_target("https://other.test/", &env, None)
            .await
            .expect("ok")
            .is_some());
    }

    #[tokio::test]
    async fn no_proxy_port_qualified_only_blocks_matching_port() {
        // `host:443` blocks the default-port https target but not an explicit :8443 one.
        let env = ctx([("https_proxy", "http://proxy:8080"), ("no_proxy", "api.example.com:443")]);
        assert!(resolve_http_proxy_url_for_target("https://api.example.com/", &env, None)
            .await
            .expect("ok")
            .is_none());
        assert!(resolve_http_proxy_url_for_target("https://api.example.com:8443/", &env, None)
            .await
            .expect("ok")
            .is_some());
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
}
