//! `ProxyStreamFn` (and custom-transport) live-Agent-turn conformance (gap-03 #4).
//!
//! gap-03 recorded that `ProxyStreamFn` — the crate's own analogue of Pi's proxy-closure `streamFn`
//! (proxy.ts:92-98) — was **never wired into any live `Agent`**, so the whole proxy transport seam
//! was unreachable. These tests drive a real [`Agent::prompt`] turn through:
//!
//!  1. a custom [`StreamFn`] spy (proving an embedder-supplied transport serves a live turn), and
//!  2. a real [`ProxyStreamFn`] against a local SSE server speaking Pi's proxy wire protocol
//!     (proxy.ts:36-57) — proving `ProxyStreamFn` streams an end-to-end live Agent turn over the wire.
//!
//! No network / tokens: transports are a scripted `FauxProvider`-backed spy or a loopback SSE server
//! on a `std::net` OS thread (the workspace `tokio` has no `net` feature, so std net is used).

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{Agent, AgentMessage, ProviderStreamFn, ProxyStreamFn, StreamFn};
use cyrup_core::{Content, EventStream, ModelRef, StopReason};
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_provider::{Context, Provider, StreamEvent, StreamOptions};

use super::support::anthropic_model_ref;

fn assistant_text(m: &AgentMessage) -> String {
    match m {
        AgentMessage::Assistant(a) => a
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

// ----------------------------------------------------------------------------------------------
// 1. A custom StreamFn serves a live Agent turn.
// ----------------------------------------------------------------------------------------------

struct HitCountingStreamFn {
    inner: ProviderStreamFn,
    hits: Arc<AtomicUsize>,
}

impl StreamFn for HitCountingStreamFn {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        self.inner.stream(model, ctx, opts)
    }
}

#[tokio::test]
async fn injected_stream_fn_serves_a_live_agent_turn() {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("reply from the injected transport")],
        StopReason::Stop,
    )]);
    let hits = Arc::new(AtomicUsize::new(0));
    let sf: Arc<dyn StreamFn> = Arc::new(HitCountingStreamFn {
        inner: ProviderStreamFn::new(faux as Arc<dyn Provider>),
        hits: hits.clone(),
    });

    let agent = Agent::builder(anthropic_model_ref(), sf).build();
    let handle = agent.prompt("hi").await.unwrap();
    let new = handle.finished().await;
    agent.wait_for_idle().await;

    assert_eq!(new.len(), 2, "user + assistant");
    assert_eq!(assistant_text(&new[1]), "reply from the injected transport");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the injected transport ran exactly once"
    );
}

// ----------------------------------------------------------------------------------------------
// 2. ProxyStreamFn streams a live Agent turn over the wire (Pi proxy protocol).
// ----------------------------------------------------------------------------------------------

/// One-shot loopback SSE server answering `POST /api/stream` with Pi-shaped proxy frames, then close.
fn spawn_proxy_server(frames: Vec<String>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("addr");
    let url = format!("http://{addr}");
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
            let mut body = String::from(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            for f in &frames {
                body.push_str("data: ");
                body.push_str(f);
                body.push_str("\n\n");
            }
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    });
    url
}

#[tokio::test]
async fn proxy_stream_fn_streams_a_live_agent_turn() {
    // Requires NO process-global `httpProxy` — see PROV-047's `PROXY_SETTING_GUARD` below.
    let _serial = PROXY_SETTING_GUARD.lock().await;
    let usage = r#"{"input":5,"output":7,"cacheRead":0,"cacheWrite":0,"totalTokens":12,"cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}"#;
    let frames = vec![
        r#"{"type":"start"}"#.to_string(),
        r#"{"type":"text_start","contentIndex":0}"#.to_string(),
        r#"{"type":"text_delta","contentIndex":0,"delta":"streamed via the proxy"}"#.to_string(),
        r#"{"type":"text_end","contentIndex":0}"#.to_string(),
        format!(r#"{{"type":"done","reason":"stop","usage":{usage}}}"#),
    ];
    let proxy_url = spawn_proxy_server(frames);

    let sf: Arc<dyn StreamFn> = Arc::new(ProxyStreamFn::new(proxy_url, "test-token"));
    let agent = Agent::builder(anthropic_model_ref(), sf).build();
    let handle = agent.prompt("ping the proxy").await.unwrap();
    let new = handle.finished().await;
    agent.wait_for_idle().await;

    assert_eq!(new.len(), 2, "user + assistant");
    assert_eq!(
        assistant_text(&new[1]),
        "streamed via the proxy",
        "ProxyStreamFn must stream the live Agent turn over the wire"
    );
}

// ----------------------------------------------------------------------------------------------
// 3. AGENT-035 — an abort mid-proxy-stream carries pi's own message.
//
// `streamProxy` checks the signal by hand and throws a LITERAL at both check points —
// `proxy.ts:186-190` (between reads) and `:208-211` (after the read loop drains) @v0.83.0 — and
// the outer catch copies that text into `partial.errorMessage` before pushing the terminal `error`
// event (`:215-223`). cyrup emitted `ProviderError::Aborted`'s bare `Display` ("aborted"), so the
// transcript of an aborted proxy turn read differently from pi's.
// ----------------------------------------------------------------------------------------------

/// A loopback SSE server that emits `frames`, then holds the connection open until the CLIENT
/// closes it. No sleep and no timer: the server blocks in `read`, which returns 0 the moment the
/// aborted request drops its response, so the thread exits on its own.
fn spawn_stalling_proxy_server(frames: Vec<String>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("addr");
    let url = format!("http://{addr}");
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let mut acc: Vec<u8> = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if acc.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => return,
                }
            }
            let mut head = String::from(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            for f in &frames {
                head.push_str("data: ");
                head.push_str(f);
                head.push_str("\n\n");
            }
            if stream.write_all(head.as_bytes()).is_err() {
                return;
            }
            let _ = stream.flush();
            // Deliberately send NO terminal frame: park until the client hangs up.
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
            }
        }
    });
    url
}

#[tokio::test]
async fn agent035_aborted_proxy_stream_reports_pis_request_aborted_by_user() {
    use cyrup_core::{CancelToken, RunCancel};
    use cyrup_provider::stream::ErrorReason;
    use futures::StreamExt;

    // Requires NO process-global `httpProxy` — see PROV-047's `PROXY_SETTING_GUARD` below.
    let _serial = PROXY_SETTING_GUARD.lock().await;

    let frames = vec![
        r#"{"type":"start"}"#.to_string(),
        r#"{"type":"text_start","contentIndex":0}"#.to_string(),
        r#"{"type":"text_delta","contentIndex":0,"delta":"partial"}"#.to_string(),
    ];
    let proxy_url = spawn_stalling_proxy_server(frames);

    let run_cancel = RunCancel::new();
    let token: CancelToken = run_cancel.token();
    let opts = crate::ProxyStreamOptions {
        auth_token: "test-token".into(),
        proxy_url,
        cancel: Some(token),
        ..Default::default()
    };
    let mut stream = crate::stream_proxy(anthropic_model_ref(), Context::default(), opts);

    // Deterministic rendezvous: cancel only once the transport has actually delivered a body
    // frame, so the abort is observed BETWEEN reads — pi's `proxy.ts:186-190` position — with no
    // wall-clock sleep anywhere in the test.
    let mut saw_delta = false;
    // A generous ceiling used as a HANG detector, not a latency assertion: a regression that stops
    // turning a cancel into a terminal event leaves the server parked and this loop awaiting
    // forever, which must surface as a failure rather than a stuck suite.
    let terminal: Option<StreamEvent> =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut terminal = None;
            while let Some(ev) = stream.next().await {
                match ev {
                    StreamEvent::TextDelta { .. } if !saw_delta => {
                        saw_delta = true;
                        run_cancel.cancel();
                    }
                    e @ StreamEvent::Error { .. } => {
                        terminal = Some(e);
                        break;
                    }
                    _ => {}
                }
            }
            terminal
        })
        .await
        .expect("a cancelled proxy stream must push its terminal error event, not hang");
    assert!(
        saw_delta,
        "the stalling server delivered its text_delta frame before the abort"
    );

    let Some(StreamEvent::Error { reason, error }) = terminal else {
        panic!("an aborted proxy stream must still push a terminal error event (proxy.ts:219-223)");
    };
    // `signal?.aborted ? "aborted" : "error"` (proxy.ts:216) — already correct before the fix.
    assert_eq!(reason, ErrorReason::Aborted);
    // RED before the fix: this was `ProviderError::Aborted`'s Display, the bare "aborted".
    assert_eq!(
        error.error_message.as_deref(),
        Some("Request aborted by user"),
        "pi puts its own literal into partial.errorMessage (proxy.ts:189/:210 → :215-218)"
    );
}

// ----------------------------------------------------------------------------------------------
// 4. PROV-047 — the proxy transport honours the `httpProxy` setting.
//
// pi's `applyHttpProxySettings` writes `process.env.HTTP_PROXY ??= proxy` (http-dispatcher.ts:43-48
// @v0.83.0) and `configureHttpDispatcher` installs an `EnvHttpProxyAgent` as the GLOBAL undici
// dispatcher that `globalThis.fetch` then runs on (`:79-93`, `:103`), so the bare `fetch` in
// `proxy.ts:165` is proxied like every other request in the process. cyrup built this transport's
// client with the proxy-BLIND `build_client()`, so an operator whose only egress is an HTTPS proxy
// got working model streaming and a hard connect failure here, with nothing in the error naming the
// proxy that was configured and ignored.
// ----------------------------------------------------------------------------------------------

/// `configure_http_proxy` is PROCESS-global, so the test that installs one must not overlap the two
/// above that require none — they would be routed into the recording proxy and fail. This
/// serializes those three tests; it weakens no assertion in any of them.
static PROXY_SETTING_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Clears the process-global `httpProxy` on the way out — in `Drop`, not on the success path, so a
/// panicking assertion (or a future `?`/cancellation at any `.await` in the test) cannot leak the
/// setting into whichever test takes [`PROXY_SETTING_GUARD`] next.
struct ClearHttpProxyOnDrop;

impl Drop for ClearHttpProxyOnDrop {
    fn drop(&mut self) {
        cyrup_provider::configure_http_proxy(None);
    }
}

/// A loopback HTTP proxy: records the request line it is handed, then answers it itself with Pi's
/// proxy SSE frames. For a plain-`http` target, reqwest sends the ABSOLUTE-form request line
/// (`POST http://host:port/api/stream HTTP/1.1`) to the proxy, which is what makes the recorded line
/// proof that the request was proxied rather than sent direct.
fn spawn_recording_http_proxy(frames: Vec<String>) -> (String, std::sync::mpsc::Receiver<String>) {
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
            let mut body = String::from(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            for f in &frames {
                body.push_str("data: ");
                body.push_str(f);
                body.push_str("\n\n");
            }
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
    });
    (url, rx)
}

#[tokio::test]
async fn prov047_the_proxy_transport_routes_through_the_configured_http_proxy() {
    let _serial = PROXY_SETTING_GUARD.lock().await;

    let usage = r#"{"input":5,"output":7,"cacheRead":0,"cacheWrite":0,"totalTokens":12,"cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}"#;
    let frames = vec![
        r#"{"type":"start"}"#.to_string(),
        r#"{"type":"text_start","contentIndex":0}"#.to_string(),
        r#"{"type":"text_delta","contentIndex":0,"delta":"streamed via the proxy"}"#.to_string(),
        r#"{"type":"text_end","contentIndex":0}"#.to_string(),
        format!(r#"{{"type":"done","reason":"stop","usage":{usage}}}"#),
    ];
    let (proxy_url, seen) = spawn_recording_http_proxy(frames);

    // The proxy TARGET is a port nothing listens on: bound only long enough for the OS to name a
    // free one, then released. A transport that ignores the setting connects here, fails, and both
    // assertions below go red — so the test cannot pass by accident.
    let target = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = l.local_addr().expect("addr").port();
        drop(l);
        format!("http://127.0.0.1:{port}")
    };

    let _restore = ClearHttpProxyOnDrop;
    cyrup_provider::configure_http_proxy(Some(proxy_url));

    // Pin `no_proxy` for this request instead of inheriting the host's.
    //
    // The proxy TARGET is necessarily loopback (the recording server above), and the ported
    // resolver honors `no_proxy` for the hop — correctly, and 1:1 with Pi. So on any machine whose
    // ambient `no_proxy` exempts loopback, the resolver declined to proxy and the turn died on the
    // dead port: the assertions below were reading the developer's shell, not PROV-047. Debian's
    // default `no_proxy` and this project's CI container both list `127.0.0.1`.
    //
    // A non-empty overlay wins over the ambient value (`node_http_proxy::get_proxy_env`), so
    // naming a host that is NOT the target pins "nothing here is exempt" hermetically, without
    // scrubbing the process env.
    let mut env = cyrup_provider::ProviderEnv::new();
    env.insert("no_proxy".to_string(), "never-matches.invalid".to_string());

    let sf: Arc<dyn StreamFn> =
        Arc::new(ProxyStreamFn::new(target.clone(), "test-token").with_env(env));
    let agent = Agent::builder(anthropic_model_ref(), sf).build();
    let handle = agent.prompt("ping through the proxy").await.unwrap();
    let new = handle.finished().await;
    agent.wait_for_idle().await;

    // Presence first: the proxy saw the request, and saw it addressed to the target in absolute
    // form — i.e. it was proxied, not merely connected to.
    let request_line = seen
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the configured httpProxy must receive the proxy-transport request");
    assert!(
        request_line.contains(&format!("{target}/api/stream")),
        "the proxy must be handed the absolute-form target URI, got {request_line:?}"
    );
    // And the turn completed THROUGH it, rather than dying on a connect error to the dead target.
    assert_eq!(new.len(), 2, "user + assistant");
    assert_eq!(
        assistant_text(&new[1]),
        "streamed via the proxy",
        "the proxied turn must stream end to end"
    );
}
