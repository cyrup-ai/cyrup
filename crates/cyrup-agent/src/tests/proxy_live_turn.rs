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
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::{Agent, AgentMessage, ProviderStreamFn, ProxyStreamFn, StreamFn};
use cyrup_core::{Content, EventStream, ModelRef, StopReason};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::{Context, Provider, StreamEvent, StreamOptions};

fn model_ref() -> ModelRef {
    ModelRef { provider: "anthropic".into(), api: Some("anthropic-messages".into()), model: "claude".into() }
}

fn assistant_text(m: &AgentMessage) -> String {
    match m {
        AgentMessage::Assistant(a) => a
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text, .. } => Some(text.clone()),
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

struct RecordingStreamFn {
    inner: ProviderStreamFn,
    hits: Arc<AtomicUsize>,
}

impl StreamFn for RecordingStreamFn {
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
    let sf: Arc<dyn StreamFn> = Arc::new(RecordingStreamFn {
        inner: ProviderStreamFn::new(faux as Arc<dyn Provider>),
        hits: hits.clone(),
    });

    let agent = Agent::builder(model_ref(), sf).build();
    let handle = agent.prompt("hi").await.unwrap();
    let new = handle.finished().await;
    agent.wait_for_idle().await;

    assert_eq!(new.len(), 2, "user + assistant");
    assert_eq!(assistant_text(&new[1]), "reply from the injected transport");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the injected transport ran exactly once");
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
    let agent = Agent::builder(model_ref(), sf).build();
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
    let mut stream = crate::stream_proxy(model_ref(), Context::default(), opts);

    // Deterministic rendezvous: cancel only once the transport has actually delivered a body
    // frame, so the abort is observed BETWEEN reads — pi's `proxy.ts:186-190` position — with no
    // wall-clock sleep anywhere in the test.
    let mut saw_delta = false;
    // A generous ceiling used as a HANG detector, not a latency assertion: a regression that stops
    // turning a cancel into a terminal event leaves the server parked and this loop awaiting
    // forever, which must surface as a failure rather than a stuck suite.
    let terminal: Option<StreamEvent> = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        async {
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
        },
    )
    .await
    .expect("a cancelled proxy stream must push its terminal error event, not hang");
    assert!(saw_delta, "the stalling server delivered its text_delta frame before the abort");

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
