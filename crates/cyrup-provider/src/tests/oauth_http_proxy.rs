//! PROV-047 — an OAuth token exchange must go through the configured `httpProxy`.
//!
//! # Why this test exists
//!
//! pi has no per-path proxy decision to get wrong. `applyHttpProxySettings` writes the setting into
//! the process environment (`process.env.HTTP_PROXY ??= proxy; process.env.HTTPS_PROXY ??= proxy`,
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
//! The OAuth half was rewired onto [`crate::stream::sse::build_client_for`], but nothing MEASURED
//! it — the guarantee rested on reading the call sites. This test measures it: a real OAuth token
//! refresh, a real loopback proxy, and a token endpoint on a dead port so that a client which
//! ignored the setting cannot pass by accident.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::auth::oauth::anthropic::AnthropicOAuth;
use crate::auth::types::Credential;
use std::io::{Read, Write};

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
    let _serial = crate::tests::proxy_setting::guard().await;

    // `TokenResponse` (anthropic.rs:285-289 ← `anthropic.ts:221-226`).
    let token_json = r#"{"access_token":"proxied-access","refresh_token":"proxied-refresh","expires_in":3600}"#;
    let (proxy_url, seen) = spawn_recording_http_proxy(token_json.to_string());

    let dead = dead_loopback_url();
    let token_url = format!("{dead}/v1/oauth/token");
    let authorize_url = format!("{dead}/oauth/authorize");

    let _restore = crate::tests::proxy_setting::ClearOnDrop;
    crate::stream::sse::configure_http_proxy(Some(proxy_url));

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
        Credential::Oauth { access, refresh, .. } => {
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
    let _serial = crate::tests::proxy_setting::guard().await;
    let _restore = crate::tests::proxy_setting::ClearOnDrop;
    crate::stream::sse::configure_http_proxy(None);

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
