#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

//! PROV-047 — an OAuth token exchange must go through the configured `httpProxy`.
//!
//! # Why this file is a SEPARATE integration test binary, not `src/tests/oauth_http_proxy.rs`
//!
//! [`cyrup_provider::stream::sse::configure_http_proxy`] writes a PROCESS-GLOBAL static
//! (`HTTP_PROXY_SETTING`) that every later `build_client_for_target` call consults as a fallback
//! (`utils/node_http_proxy.rs`'s `get_proxy_env`, PROV-047). That is intentional, correct parity
//! with pi's `applyHttpProxySettings` writing `process.env.HTTP_PROXY` — but it makes any test that
//! calls `configure_http_proxy(Some(..))` a hazard to every OTHER concurrently-running test in the
//! same OS process that builds a client without its own explicit `http_proxy`/`https_proxy`
//! override (which is most of this crate's loopback-mock tests, since they use `EmptyEnv` or a
//! `MapEnv` with no proxy key). `src/tests/proxy_setting.rs`'s `guard()` mutex only serializes the
//! tests that themselves call it — it cannot protect a bystander test that has no idea this file
//! exists.
//!
//! This was a real, reproduced bug, not a theoretical one: with this file inside the crate's single
//! shared unit-test binary (as `src/tests/oauth_http_proxy.rs`, alongside `src/tests/mod.rs`'s
//! other modules), `cargo test -p cyrup-provider` at the default thread count reliably produced
//! `ECONNREFUSED` in unrelated `remote_catalog`/`github_copilot` loopback tests, every one of them
//! showing a connect target that was neither their own mock origin NOR any explanation involving a
//! proxy or DNS leak through `reqwest`/`hyper` (both audited and ruled out) — it was this file's own
//! `spawn_recording_http_proxy` address, silently inherited via the global fallback while THIS
//! file's test held the setting active. Moving this file here — a genuine, separate OS process per
//! `cargo test` integration-test binary — makes `HTTP_PROXY_SETTING` (a `static` in the LINKED
//! library, not shared across processes) invisible to every other test, present or future, with no
//! per-test opt-in required anywhere else. `src/tests/mod.rs` still says the other modules were
//! deliberately consolidated into one binary for build/CI speed; this is a narrow, justified
//! exception for the one module that mutates real process-global state.
//!
//! ## What this test measures
//!
//! pi has no per-path proxy decision to get wrong: `applyHttpProxySettings` writes into the process
//! environment (`process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`,
//! `coding-agent/src/core/http-dispatcher.ts:43-48` @v0.83.0) and `configureHttpDispatcher` installs
//! an `EnvHttpProxyAgent` as the GLOBAL undici dispatcher, then swaps `globalThis.fetch` onto that
//! same undici (`:79-105`). Both call sites do the pair together — the bootstrap one at
//! `coding-agent/src/main.ts:537-538` and the post-runtime one at `:801-802`. So in pi EVERY `fetch`
//! in the process is proxied, and the OAuth flows (`ai/src/auth/oauth/anthropic.ts:206`'s bare
//! `fetch` to the token endpoint) inherit that for free.
//!
//! cyrup has no ambient `fetch` and cannot write its own environment (`std::env::set_var` is
//! `unsafe` from edition 2024), so the coverage pi gets by construction has to be assembled one
//! client at a time — which is exactly how PROV-047 came to be a real bug: the streaming wire APIs
//! were proxy-aware and the OAuth clients were built with the proxy-BLIND `build_client()`. A user
//! who set `httpProxy` because their network requires it got a working chat and a silently-direct
//! login.
//!
//! The OAuth half was rewired onto [`cyrup_provider::stream::sse::build_client_for_target`], but
//! nothing MEASURED it — the guarantee rested on reading the call sites. This test measures it: a
//! real OAuth token refresh, a real loopback proxy, and a token endpoint on a dead port so that a
//! client which ignored the setting cannot pass by accident.

use cyrup_provider::auth::oauth::anthropic::AnthropicOAuth;
use cyrup_provider::auth::types::Credential;
use std::io::{Read, Write};

/// Serializes this file's OWN two tests against EACH OTHER (both write the same process-global
/// setting). Unlike `src/tests/proxy_setting.rs`'s crate-wide `guard()`, this one only ever needs
/// to cover the tests in this file, because moving the file already removes every OTHER test in the
/// crate from the blast radius — that is the whole point of the move documented above.
static PROXY_SETTING_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Clears the process-global `httpProxy` in `Drop` — never on the success path, so a panicking
/// assertion cannot leak the setting into whichever of this file's two tests runs next.
struct ClearOnDrop;

impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        cyrup_provider::stream::sse::configure_http_proxy(None);
    }
}

/// A loopback HTTP proxy: records the request line it is handed, then answers it itself with the
/// canned token JSON. For a plain-`http` target, `reqwest` sends the ABSOLUTE-form request line
/// (`POST http://host:port/token HTTP/1.1`) to the proxy — which is what makes the recorded line
/// proof that the request was PROXIED rather than sent direct, since a direct request would carry
/// only the origin-form path (`POST /token HTTP/1.1`) and would go to the other socket entirely.
fn spawn_recording_http_proxy(response_body: String) -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("addr");
    let url = format!("http://{addr}");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let mut acc: Vec<u8> = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let head = String::from_utf8_lossy(&acc);
            let _ = tx.send(head.lines().next().unwrap_or_default().to_string());
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
    });
    (url, rx)
}

/// A loopback address nothing is listening on: bound only long enough for the OS to name a free
/// port, then released. Any client that ignores the `httpProxy` setting connects HERE, fails, and
/// both assertions below go red.
fn dead_loopback_url() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = l.local_addr().expect("addr").port();
    drop(l);
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn prov047_an_oauth_token_refresh_routes_through_the_configured_http_proxy() {
    let _serial = PROXY_SETTING_GUARD.lock().await;

    // `TokenResponse` (anthropic.rs:285-289 ← `anthropic.ts:221-226`).
    let token_json = r#"{"access_token":"proxied-access","refresh_token":"proxied-refresh","expires_in":3600}"#;
    let (proxy_url, seen) = spawn_recording_http_proxy(token_json.to_string());

    let dead = dead_loopback_url();
    let token_url = format!("{dead}/v1/oauth/token");
    let authorize_url = format!("{dead}/oauth/authorize");

    let _restore = ClearOnDrop;
    cyrup_provider::stream::sse::configure_http_proxy(Some(proxy_url));

    let flow = AnthropicOAuth::with_endpoints(&authorize_url, &token_url, "127.0.0.1", 0);
    let credential = flow
        .refresh_token("stored-refresh-token")
        .await
        .expect("the refresh must succeed THROUGH the configured proxy");

    // Presence first: the proxy was handed the request, in absolute form, addressed at the token
    // endpoint — i.e. it was proxied, not merely connected to.
    let request_line = seen
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the configured httpProxy must receive the OAuth token request");
    assert!(
        request_line.contains(&token_url),
        "the proxy must be handed the absolute-form token URI, got {request_line:?}"
    );
    assert!(
        request_line.starts_with("POST "),
        "the OAuth token exchange is a POST (anthropic.ts:198-205), got {request_line:?}"
    );

    // ...and the flow really consumed the proxy's response, rather than the assertion above passing
    // on a request that then died.
    match credential {
        Credential::Oauth {
            access, refresh, ..
        } => {
            assert_eq!(access, "proxied-access");
            assert_eq!(refresh, "proxied-refresh");
        }
        other => panic!("an OAuth refresh must yield an OAuth credential, got {other:?}"),
    }
}

/// The negative half, asserted after the positive one: with NO setting configured and no ambient
/// proxy for this target, the same flow talks to the token endpoint DIRECTLY. Without this, the
/// test above would also pass for a client that proxied unconditionally — which would be its own
/// bug (`NO_PROXY`/no-setting must still mean a direct connection, `node-http-proxy.ts:92-112`).
#[tokio::test]
async fn prov047_with_no_proxy_configured_the_same_flow_connects_directly() {
    let _serial = PROXY_SETTING_GUARD.lock().await;
    let _restore = ClearOnDrop;
    cyrup_provider::stream::sse::configure_http_proxy(None);

    // Here the recording listener IS the token endpoint, so a direct connection reaches it and the
    // request line arrives in ORIGIN form — the shape a non-proxied request has on the wire.
    let token_json = r#"{"access_token":"direct-access","refresh_token":"direct-refresh","expires_in":3600}"#;
    let (endpoint, seen) = spawn_recording_http_proxy(token_json.to_string());
    let token_url = format!("{endpoint}/v1/oauth/token");

    let flow = AnthropicOAuth::with_endpoints(
        format!("{endpoint}/oauth/authorize"),
        &token_url,
        "127.0.0.1",
        0,
    );
    let credential = flow
        .refresh_token("stored-refresh-token")
        .await
        .expect("the refresh must succeed directly");

    let request_line = seen
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the token endpoint must receive the request directly");
    assert_eq!(
        request_line, "POST /v1/oauth/token HTTP/1.1",
        "an unproxied request must use the ORIGIN-form request line, got {request_line:?}"
    );
    match credential {
        Credential::Oauth { access, .. } => assert_eq!(access, "direct-access"),
        other => panic!("expected an OAuth credential, got {other:?}"),
    }
}

// ------------------------------------------------------------------------------ resolver-level ---

/// An `AuthContext` over a fixed map, for exercising `resolve_http_proxy_url_for_target` directly
/// (moved here from `crates/cyrup-provider/src/utils/node_http_proxy.rs`'s inline unit tests — see
/// this file's module doc comment for why any test that calls `configure_http_proxy` cannot safely
/// live in the crate's shared unit-test binary).
struct MapEnv(std::collections::BTreeMap<String, String>);

#[async_trait::async_trait]
impl cyrup_provider::auth::types::AuthContext for MapEnv {
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

/// The `httpProxy` SETTING must reach the resolver, not just the `provider_env` overlay the
/// streaming APIs happen to receive. Upstream this is automatic because `applyHttpProxySettings`
/// writes `process.env.HTTP_PROXY` (`coding-agent/src/core/http-dispatcher.ts:43-48`); here it is
/// `sse::configure_http_proxy`, consulted at the same layer.
///
/// These four assertions run as one test on purpose: the setting is process-global, so two
/// `#[test]`s writing it would race if they shared a binary — which is exactly why this whole file
/// is its own `cargo test` process (see the module doc comment) rather than relying on a mutex to
/// protect bystanders that don't know to acquire it.
#[tokio::test]
async fn the_http_proxy_setting_reaches_the_resolver_and_yields_to_everything_above_it() {
    let _serial = PROXY_SETTING_GUARD.lock().await;
    let _restore = ClearOnDrop;
    let none = ctx([]);

    // (1) Unset ⇒ unchanged behaviour: no setting, no ambient var, no proxy.
    cyrup_provider::stream::sse::configure_http_proxy(None);
    assert!(
        cyrup_provider::utils::node_http_proxy::resolve_http_proxy_url_for_target(
            "https://api.example.com/",
            &none,
            None
        )
        .await
        .expect("ok")
        .is_none()
    );

    // (2) Set ⇒ the request that previously bypassed the proxy now uses it. This is the whole of
    // PROV-047's user-visible failure: OAuth login on a proxy-only network.
    cyrup_provider::stream::sse::configure_http_proxy(Some("http://corp-proxy:3128".to_string()));
    let out = cyrup_provider::utils::node_http_proxy::resolve_http_proxy_url_for_target(
        "https://api.example.com/",
        &none,
        None,
    )
    .await
    .expect("ok")
    .expect("the configured proxy applies");
    assert_eq!(out.host_str(), Some("corp-proxy"));
    assert_eq!(out.port(), Some(3128));

    // (3) `??=` — an ambient variable WINS over the setting.
    let ambient = ctx([("https_proxy", "http://ambient:9")]);
    let out = cyrup_provider::utils::node_http_proxy::resolve_http_proxy_url_for_target(
        "https://api.example.com/",
        &ambient,
        None,
    )
    .await
    .expect("ok")
    .expect("proxy");
    assert_eq!(
        out.host_str(),
        Some("ambient"),
        "process.env.HTTPS_PROXY ??= proxy does not overwrite an existing value"
    );

    // (4) `NO_PROXY` still bypasses — the setting is a value for HTTP(S)_PROXY, not an override of
    // the resolver. PROV-047's negative Verify clause.
    let bypass = ctx([("no_proxy", "api.example.com")]);
    assert!(
        cyrup_provider::utils::node_http_proxy::resolve_http_proxy_url_for_target(
            "https://api.example.com/",
            &bypass,
            None
        )
        .await
        .expect("ok")
        .is_none(),
        "NO_PROXY must beat the configured setting"
    );

    cyrup_provider::stream::sse::configure_http_proxy(None);
}

