//! Settings-string parsing and the HTTP-proxy overlay — the free helpers that turn a settings
//! value into the shape a subsystem takes, with no dependence on `SessionBuilder`'s state.

/// Build the provider-scoped env overlay for the configured HTTP proxy (Pi `applyHttpProxySettings`,
/// http-dispatcher.ts:42-47): a non-empty `httpProxy` setting sets both `HTTP_PROXY` and `HTTPS_PROXY`
/// in the overlay (matching Pi's `process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`),
/// so the provider's `resolveHttpProxyUrlForTarget` routes requests through it. An absent/blank setting
/// yields `None` (the ambient process env is used unchanged).
/// Pi `applyHttpProxySettings(settings.httpProxy)` — BOTH of its layers (PROV-047).
///
/// Upstream this is a single function that writes the **process environment**
/// (`process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`,
/// `coding-agent/src/core/http-dispatcher.ts:43-48` @v0.83.0, `:45-50` @v0.84.1), called at startup
/// from `cli.ts:18` / `rpc-entry.ts:10` and re-applied from `main.ts:744`. Because the value lands
/// in the env, EVERY later proxy consultation in the process observes it — the global undici
/// dispatcher (`:79-93`) that `globalThis.fetch` runs on (`:103`), OAuth token exchange, silent
/// token refresh, catalog refreshes, the agent proxy transport, extension HTTP — and
/// `node-http-proxy.ts`'s per-request resolution is a SECOND layer on top, not a replacement.
///
/// cyrup needs both layers explicitly:
///
/// 1. [`cyrup_provider::configure_http_proxy`] is the stand-in for pi's env write
///    (`std::env::set_var` is `unsafe` from edition 2024 and races every concurrently running
///    thread's `getenv`). It is consulted by the ported resolver at exactly the layer pi's env
///    write is observed, so the `??=` precedence survives: an ambient `HTTP_PROXY`/`HTTPS_PROXY`
///    still wins. It is called **unconditionally, including with `None`**, so clearing the setting
///    clears the global rather than leaving the previous value installed.
/// 2. [`http_proxy_overlay`] is pi's second layer — the provider-scoped `StreamOptions.env`
///    returned here for the caller to attach, which is what reaches the streaming wire APIs.
///
/// Layer 1 alone was missing until PROV-047, which is why `httpProxy` reached the streams and
/// nothing else.
pub(super) fn apply_http_proxy_settings(
    proxy: Option<String>,
) -> Option<cyrup_provider::ProviderEnv> {
    cyrup_provider::configure_http_proxy(proxy.clone());
    http_proxy_overlay(proxy.as_deref())
}

fn http_proxy_overlay(proxy: Option<&str>) -> Option<cyrup_provider::ProviderEnv> {
    let proxy = proxy?.trim();
    if proxy.is_empty() {
        return None;
    }
    let mut overlay = cyrup_provider::ProviderEnv::new();
    overlay.insert("HTTP_PROXY".to_string(), proxy.to_string());
    overlay.insert("HTTPS_PROXY".to_string(), proxy.to_string());
    Some(overlay)
}

/// Parse the settings `steeringMode`/`followUpMode` string into the agent's [`QueueMode`]
/// (Pi `"all"|"one-at-a-time"`; settings-manager.ts:698-710). Any non-`all` value ⇒ one-at-a-time.
pub(crate) fn parse_queue_mode(s: &str) -> cyrup_agent::QueueMode {
    if s == "all" { cyrup_agent::QueueMode::All } else { cyrup_agent::QueueMode::OneAtATime }
}

/// Parse the settings `transport` string into the provider [`Transport`] Pi hands the agent
/// (`sdk.ts:357` `transport: settingsManager.getTransport()`; the `TransportSetting` union is
/// `"auto" | "sse" | "websocket" | "websocket-cached"`, types.ts:98). The strings are byte-1:1 with
/// Pi because `Transport` is `#[serde(rename_all = "kebab-case")]`. An unrecognized value falls back
/// to `auto`, matching `getTransport()`'s `?? "auto"` and the settings dialog's fixed choice set.
pub(crate) fn parse_transport(s: &str) -> cyrup_provider::Transport {
    match s {
        "sse" => cyrup_provider::Transport::Sse,
        "websocket" => cyrup_provider::Transport::Websocket,
        "websocket-cached" => cyrup_provider::Transport::WebsocketCached,
        _ => cyrup_provider::Transport::Auto,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::http_proxy_overlay;

    #[test]
    fn http_proxy_overlay_sets_both_proxy_keys_or_none() {
        // Pi `applyHttpProxySettings` (http-dispatcher.ts:42-47): a non-empty setting sets both
        // HTTP_PROXY and HTTPS_PROXY (so the provider proxy resolver routes through it).
        let overlay = http_proxy_overlay(Some("http://proxy.local:8080")).expect("an overlay");
        assert_eq!(overlay.get("HTTP_PROXY").map(String::as_str), Some("http://proxy.local:8080"));
        assert_eq!(overlay.get("HTTPS_PROXY").map(String::as_str), Some("http://proxy.local:8080"));
        // A blank / whitespace / absent setting yields no overlay (ambient env unchanged).
        assert!(http_proxy_overlay(Some("   ")).is_none());
        assert!(http_proxy_overlay(Some("")).is_none());
        assert!(http_proxy_overlay(None).is_none());
    }

    /// PROV-047 — the setting must reach pi's FIRST layer too, the one every non-streaming egress
    /// path consults. Before the fix, `build()` attached the overlay and never called
    /// `configure_http_proxy`, so `httpProxy` reached the streaming wire APIs and nothing else:
    /// OAuth login, silent token refresh, catalog refreshes, the agent proxy transport and
    /// extension HTTP all connected direct and failed on a proxy-only network, with nothing in the
    /// error naming the proxy that had been configured and ignored.
    ///
    /// The clearing half is asserted as well: `applyHttpProxySettings` is re-invoked on every
    /// rebuild (pi calls it again from `main.ts:744`), so a setting that was removed must not leave
    /// the previous proxy installed process-wide.
    #[test]
    fn apply_http_proxy_settings_installs_the_process_global_not_just_the_overlay() {
        let overlay = super::apply_http_proxy_settings(Some("http://proxy.local:8080".to_string()))
            .expect("a non-empty setting still yields pi's second-layer overlay");
        assert_eq!(overlay.get("HTTPS_PROXY").map(String::as_str), Some("http://proxy.local:8080"));
        assert_eq!(
            cyrup_provider::configured_http_proxy().as_deref(),
            Some("http://proxy.local:8080"),
            "the setting must also be installed process-wide, or it reaches only the streams"
        );

        assert!(super::apply_http_proxy_settings(None).is_none());
        assert_eq!(
            cyrup_provider::configured_http_proxy(),
            None,
            "clearing the setting must clear the global, not leave the previous proxy installed"
        );
    }
}
