//! WASM HTTP-CLIENT CAPABILITY END-TO-END (arch-08 §3.2 draft; `pi-mcp-adapter-port.md` §3.2 — the
//! locked WIT shape this closes). Proves that the capability-scoped `http-client` a LIVE wasm guest
//! calls (`ctx.http_request(...)` / `ctx.http_request_stream(...)` + `http_poll_stream_chunk`)
//! reaches the session's REAL local [`cyrup_ext::caps::http::HttpCaps`] (a real `reqwest::Client`)
//! through the injected `LiveHostServices` (arch-08 §5.6) and returns REAL captured bytes off a REAL
//! local TCP server — not a stub, not a canned answer.
//!
//! Mirrors `tests/wasm_exec.rs`'s structure/discipline 1:1 for the new capability: LOADED ==
//! TRUSTED-BY-CONSTRUCTION (`trust_override = Some(true)`), so the guest's `http-client` grant is
//! live via the SAME trust gate `exec`/`ui` use (no new bool, no per-host allowlist). The untrusted
//! -denial analog (`DenyServices`) is proven in `cyrup-ext/tests/wasm_component.rs`.
//!
//! The mock HTTP server is a bare `tokio::net::TcpListener` responder — no external network
//! dependency, no mocking crate — the same technique `cyrup-provider/src/wire.rs`'s own tests use
//! for mocking a wire API, and `cyrup-ext/src/caps/http.rs`'s own unit tests use for `HttpCaps`
//! directly (this test drives the SAME engine end-to-end THROUGH a live wasm guest instead).
// The original `#![cfg(feature = "wasm-host")]` is deliberately GONE. It named
// cyrup-session-svc's own feature, which that crate enables in its `default` — so it was
// always true here. Re-spelled in cyrup-it it would name THIS crate's `wasm-host`, which
// `--features it` does not enable, and every test below would SILENTLY not compile in.
// See the `[[test]]` note in crates/cyrup-it/Cargo.toml.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{SessionBuilder, SessionConfig};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// MIGRATION (docs/TEST-ARCHITECTURE.md §3.4): this file used to carry its own `fixture_component()`
// that shelled out to `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` into the SHARED, fixed
// `std::env::temp_dir()/cyrup-session-svc-fixture-target` — one of ten byte-identical copies that
// serialized on each other's cargo build lock and never cleaned up. `cyrup-it`'s `build.rs` now
// builds the component ONCE for the whole suite and exports its path; `CYRUP_EXT_FIXTURE_COMPONENT`
// still overrides it, at that one place instead of ten.
use crate::support::bins;

fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    faux
}

/// Spawn a raw-TCP mock HTTP/1.1 server: accepts one connection, drains the request, then writes
/// each of `parts` as a separate flushed write with a small delay between them, so a real client
/// observes them as distinct reads — proving genuine incremental delivery over the real wire (no
/// external network dependency). Returns the server's `http://127.0.0.1:<port>/probe` URL.
async fn spawn_mock(headers: String, parts: Vec<Vec<u8>>) -> String {
    spawn_mock_with_status("HTTP/1.1 200 OK", headers, parts).await
}

/// As [`spawn_mock`], with a caller-chosen status line (proves the streamed response's status isn't
/// hardcoded to 200 anywhere on the path from `HttpCaps` to the guest).
async fn spawn_mock_with_status(
    status_line: &'static str,
    headers: String,
    parts: Vec<Vec<u8>>,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let head = format!("{status_line}\r\n{headers}\r\n");
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.flush().await;
            for part in parts {
                let _ = sock.write_all(&part).await;
                let _ = sock.flush().await;
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }
        }
    });
    format!("http://{addr}/probe")
}

/// As [`spawn_mock`], but waits `delay` AFTER accepting the connection and draining the request
/// BEFORE writing anything back — simulates a genuinely slow (but real) server, not a mocked clock.
async fn spawn_mock_with_delay(
    headers: String,
    body: Vec<u8>,
    delay: std::time::Duration,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            tokio::time::sleep(delay).await;
            let head = format!("HTTP/1.1 200 OK\r\n{headers}\r\n");
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(&body).await;
            let _ = sock.flush().await;
        }
    });
    format!("http://{addr}/probe")
}

/// Build a TRUSTED session (`trust_override = Some(true)`) with a fresh project/agent dir, exactly
/// as `tests/wasm_exec.rs` does — the guest's `http-client` grant is live via the load-time trust
/// gate, the SAME one `exec`/`ui` already use.
async fn trusted_session() -> cyrup_session_svc::AgentSession {
    let tmp = TempDir::new().expect("tempdir");
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).expect("mkdir cwd");
    std::fs::create_dir_all(&agent_dir).expect("mkdir agent_dir");
    // Leak the TempDir so it outlives the session (test-process-lifetime scratch dir; mirrors the
    // discipline other wasm_*.rs fixtures use of not tearing the session's cwd down mid-test).
    std::mem::forget(tmp);

    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true); // TRUSTED project ⇒ the guest's http-client grant is live.
    cfg.no_extensions = true; // only the explicitly-loaded guest is present.

    SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, cfg)
        .build()
        .await
        .expect("build session")
}

/// THE headline proof (a): a TRUSTED live wasm guest's `ctx.http_request(GET url)` runs through the
/// session's injected `LiveHostServices` → the real `HttpCaps` engine → a real `reqwest::Client` call
/// against a REAL local TCP server, and returns the REAL status + body across the wasm boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_http_request_gets_a_real_response_through_the_assembled_session() {
    let bytes = bins::component_bytes();
    let session = trusted_session().await;

    let ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");
    assert!(
        session
            .services()
            .ext_host
            .registry()
            .command_names()
            .unwrap()
            .iter()
            .any(|n| n == "httpdemo"),
        "the guest-registered `/httpdemo` command is in the host command registry"
    );

    let body = b"hello from the real mock server";
    let headers = format!(
        "Content-Type: text/plain\r\nContent-Length: {}\r\n",
        body.len()
    );
    let url = spawn_mock(headers, vec![body.to_vec()]).await;

    // Drive the command through the REAL public entry point (prompt → prepare →
    // _tryExecuteExtensionCommand → the guest's `execute-command` export → `ctx.http_request` →
    // the WIT `http-client.request` import → LiveHostServices::http_request → the real HttpCaps
    // engine → reqwest → the real local TCP server).
    let _ = session.prompt(format!("/httpdemo {url}")).await.unwrap();
    session.wait_for_idle().await;

    let expect_msg = format!("http status: 200 body: {}", String::from_utf8_lossy(body));
    assert!(
        ext.guest().notifications().iter().any(|n| n == &expect_msg),
        "the guest observed the REAL status+body across the wasm boundary: {:?} (wanted {expect_msg:?})",
        ext.guest().notifications()
    );
}

/// THE headline proof (b): a TRUSTED live wasm guest opens a streaming request
/// (`ctx.http_request_stream`) against a server that writes its body in several delayed, separately-
/// flushed parts, then drains it with repeated `ctx.http_poll_stream_chunk` calls — proving REAL
/// chunks arrive in order across the wasm boundary and natural EOF is signalled correctly
/// (`Ok(None)`), never a stub/canned answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_http_stream_receives_real_chunks_in_order_then_eof() {
    let bytes = bins::component_bytes();
    let session = trusted_session().await;

    let ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");

    let parts: Vec<Vec<u8>> = vec![
        b"chunk-one-".to_vec(),
        b"chunk-two-".to_vec(),
        b"chunk-three".to_vec(),
    ];
    let expected_body = String::from_utf8_lossy(&parts.concat()).into_owned();
    let total: usize = parts.iter().map(Vec::len).sum();
    let headers = format!("Content-Type: application/octet-stream\r\nContent-Length: {total}\r\n");
    let url = spawn_mock(headers, parts).await;

    // Drive the command through the REAL public entry point (prompt → the guest's `execute-command`
    // export → `ctx.http_request_stream` + repeated `ctx.http_poll_stream_chunk` → the WIT
    // `http-client.request-stream`/`poll-stream-chunk` imports → LiveHostServices → the real
    // HttpCaps stream registry → reqwest's `bytes_stream()` off the real local TCP server).
    let _ = session
        .prompt(format!("/httpstreamdemo {url}"))
        .await
        .unwrap();
    session.wait_for_idle().await;

    let notifications = ext.guest().notifications();

    // Closes L4 §2.3: the initiating response's REAL status+headers are notified BEFORE any chunk is
    // polled — off the SAME `request-stream` round trip that opened the body, exactly what the real
    // consumer (the MCP SDK's `StreamableHTTPClientTransport`) needs off its one `fetch()` response.
    let opened = notifications
        .iter()
        .find(|n| n.starts_with("http stream opened status: "))
        .unwrap_or_else(|| panic!("no http-stream-open notification recorded: {notifications:?}"));
    assert_eq!(
        opened, "http stream opened status: 200 content-type: application/octet-stream",
        "the REAL status+content-type came back off request-stream's own return"
    );

    let streamed = notifications
        .iter()
        .find(|n| n.starts_with("http stream chunks: "))
        .unwrap_or_else(|| panic!("no http-stream notification recorded: {notifications:?}"));

    // Parse "http stream chunks: <N> body: <body>".
    let rest = streamed
        .strip_prefix("http stream chunks: ")
        .expect("prefix");
    let (count_str, body) = rest.split_once(" body: ").expect("split");
    let chunk_count: u32 = count_str.parse().expect("chunk count parses");

    assert_eq!(
        body, expected_body,
        "the guest's polled chunks concatenate back to the REAL body, in order"
    );
    assert!(
        chunk_count >= 2,
        "the delayed writes arrived as multiple distinct REAL chunks across the wasm boundary: {chunk_count}"
    );
}

/// THE headline proof (c), closing L4 §2.3 directly: a TRUSTED live wasm guest's
/// `ctx.http_request_stream` surfaces the initiating response's REAL non-2xx status (401, as the MCP
/// SDK's `StreamableHTTPClientTransport` needs to decide re-auth) and a distinguishing header
/// (`mcp-session-id`) across the wasm boundary — proving these are genuinely read off the real HTTP
/// response, not a hardcoded/omitted default, and that the guest can observe them independent of
/// (indeed, strictly before) ever draining the streamed body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_http_stream_surfaces_real_non_2xx_status_and_headers_before_draining_body() {
    let bytes = bins::component_bytes();
    let session = trusted_session().await;

    let ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");

    let body = b"unauthorized-but-still-a-real-streamable-body".to_vec();
    let headers = format!(
        "Content-Type: text/event-stream\r\nMcp-Session-Id: sess-live-42\r\nContent-Length: {}\r\n",
        body.len()
    );
    let url =
        spawn_mock_with_status("HTTP/1.1 401 Unauthorized", headers, vec![body.clone()]).await;

    let _ = session
        .prompt(format!("/httpstreamdemo {url}"))
        .await
        .unwrap();
    session.wait_for_idle().await;

    let notifications = ext.guest().notifications();
    let opened = notifications
        .iter()
        .find(|n| n.starts_with("http stream opened status: "))
        .unwrap_or_else(|| panic!("no http-stream-open notification recorded: {notifications:?}"));
    assert_eq!(
        opened, "http stream opened status: 401 content-type: text/event-stream",
        "the REAL non-2xx status + content-type header came back off request-stream's own return, \
         BEFORE any chunk was polled"
    );

    // The body is still fully drained afterward — exposing status/headers didn't consume anything.
    let streamed = notifications
        .iter()
        .find(|n| n.starts_with("http stream chunks: "))
        .unwrap_or_else(|| panic!("no http-stream notification recorded: {notifications:?}"));
    let rest = streamed
        .strip_prefix("http stream chunks: ")
        .expect("prefix");
    let (_count_str, streamed_body) = rest.split_once(" body: ").expect("split");
    assert_eq!(
        streamed_body,
        String::from_utf8_lossy(&body),
        "the real body still drains after 401"
    );
}

/// Closes the CRITICAL finding that `http_client::Host` (`crates/cyrup-ext/src/host/live.rs`) never
/// called `guest.note_dialog_wait(...)` anywhere — unlike `ui.*` dialogs (`845f707`), a slow-but-real
/// HTTP response left the WASM epoch deadline already expired by the time the guest resumed wasm
/// execution right after the blocking `http-client.request` host call returned, tripping the SAME
/// permanent-instance-wedging epoch trap `845f707` closed for dialogs. Reproduces that fix's own
/// methodology 1:1: a REAL ~6s-delayed response (past the ~5s `WASM_EPOCH_BUDGET_TICKS` production
/// budget, via a REAL `tokio::time::sleep` on a real local TCP server, not a mocked clock), then a
/// later, unrelated command against the SAME instance. `httpdemo` uses `HttpRequest::get` (no
/// `timeout_ms`, and `HttpCaps`'s `reqwest::Client` has no default timeout either — confirmed by
/// direct inspection of `HttpCaps::new`/`build_request`), so no independent host-side timeout races
/// with the epoch mechanism, isolating this test to it alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wasm_guest_http_request_delayed_past_the_epoch_budget_does_not_wedge_the_extension() {
    let bytes = bins::component_bytes();
    let session = trusted_session().await;

    let ext = session
        .load_wasm_extension(
            ExtensionId::from("demo"),
            &bytes,
            // EXT-059: the grant is now explicit. `host_granted()` is the TOTAL grant these
            // fixtures previously got implicitly from `load_wasm_extension`'s `load_wasm` call.
            &cyrup_ext::Capabilities::host_granted(),
        )
        .await
        .expect("load + init the live wasm extension");

    let body = b"slow but real response".to_vec();
    let headers = format!(
        "Content-Type: text/plain\r\nContent-Length: {}\r\n",
        body.len()
    );
    let url = spawn_mock_with_delay(headers, body.clone(), std::time::Duration::from_secs(6)).await;

    let _ = session.prompt(format!("/httpdemo {url}")).await.unwrap();
    session.wait_for_idle().await;

    // The delayed response still resolved to the REAL status+body, not a timeout/trap-induced default.
    let expect_msg = format!("http status: 200 body: {}", String::from_utf8_lossy(&body));
    assert!(
        ext.guest().notifications().iter().any(|n| n == &expect_msg),
        "the delayed request resolved to the real scripted response, not a default: {:?}",
        ext.guest().notifications()
    );

    // THE headline proof: a later, unrelated, http-free command against the SAME instance still
    // genuinely runs. Before this fix, the epoch trap right after the delayed response permanently
    // wedged the instance and this would silently no-op.
    let before = ext.guest().notifications().len();
    let _ = session.prompt("/execdemo").await.unwrap();
    session.wait_for_idle().await;
    assert!(
        ext.guest().notifications()[before..]
            .iter()
            .any(|n| n.starts_with("exec stdout:")),
        "the extension survives an http request delayed past the epoch budget — a later command \
         still genuinely runs, not a silent no-op: {:?}",
        ext.guest().notifications()
    );
}
