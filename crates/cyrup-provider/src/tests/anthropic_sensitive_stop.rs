//! Anthropic `stop_reason: "sensitive"` must reach the transcript as its own diagnostic.
//!
//! Upstream, verified at the ported tag with `git -C pi show
//! v0.83.0:packages/ai/src/api/anthropic-messages.ts`:
//!
//! ```text
//! case "sensitive": // Content flagged by safety filters (not yet in SDK types)
//!     return { stopReason: "error", errorMessage: "Provider stopped with: sensitive" };
//! ```
//!
//! and the string reaches the user at anthropic-messages.ts:755
//! `throw new Error(output.errorMessage || "An unknown error occurred");` — the `||` fallback fires
//! only when the mapping supplied no message. cyrup's `map_stop_reason` returned
//! `(StopReason::Error, None)` for `sensitive`, so cyrup took that fallback and a content-policy
//! stop was indistinguishable from a transport failure.
//!
//! These tests drive the **production** path — `AnthropicMessagesApi::run`, the `ApiImpl` the
//! registry hands every Anthropic turn (`api/mod.rs:128` `register_builtins`) — not the private
//! `map_stop_reason` helper. The wire bytes come off a raw loopback `std::net::TcpListener`, the
//! established no-network technique in this workspace (`cyrup-agent/tests/proxy_live_turn.rs:92`,
//! `cyrup-provider/tests/remote_catalog.rs:15`), and the request carries a `no_proxy: "*"` provider
//! env overlay so an ambient `HTTP_PROXY` on the developer's machine cannot reroute a loopback
//! request into the network.
//!
//! `refusal_carries_its_explanation` and `unknown_stop_reason_names_itself` are **mirror** cases:
//! they exercise the identical harness against the two sibling arms of the same `match` that always
//! carried text. They stay green whether or not the `sensitive` arm is fixed, which is what shows
//! the failing assertion in `sensitive_stop_reason_reaches_the_transcript_as_its_own_diagnostic` is
//! about the mapping and not about a broken fixture.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{Read, Write};

use cyrup_core::{CancelToken, Content, Message, StopReason};
use crate::{
    ApiImpl, AnthropicMessagesApi, AuthResult, Context, Modality, Model, ModelCost, ProviderEnv,
    StreamEvent, StreamOptions, channel,
};

// ------------------------------------------------------------------------------ loopback server --

/// `true` once `acc` holds a complete HTTP/1.1 request: the head, plus a `Content-Length` body when
/// one is declared. `send_once` (`stream/sse.rs:253`) builds the request with `.json(body)`, which
/// always sets `Content-Length`, so this never has to speak chunked.
fn request_is_complete(acc: &[u8]) -> bool {
    let Some(head_end) = acc.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head_end = head_end + 4;
    let head = String::from_utf8_lossy(&acc[..head_end]).to_lowercase();
    let len = head.lines().find_map(|line| {
        line.strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    match len {
        Some(n) => acc.len() >= head_end + n,
        None => true,
    }
}

/// Serve `sse_body` as one `text/event-stream` response, for as many connections as arrive.
///
/// Robustness notes, because this runs alongside 270+ suites under CPU contention:
///  * the listener loops rather than serving one shot, so a retry inside `open_sse` still lands;
///  * the request is fully drained before the response is written, so the client can never be
///    blocked writing while the server is blocked reading;
///  * the stream is closed with an explicit `shutdown(Write)` rather than a `sleep`, so the SSE
///    reader observes EOF deterministically instead of on a timer.
fn spawn_sse_server(sse_body: String) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let url = format!("http://{addr}");
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(20)));
            let mut buf = [0u8; 8192];
            let mut acc: Vec<u8> = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if request_is_complete(&acc) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let mut resp = String::from(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            resp.push_str(&sse_body);
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    });
    url
}

// ------------------------------------------------------------------------------------- fixtures --

fn model(base_url: String) -> Model {
    Model {
        id: "claude-opus-4-5".into(),
        name: "Claude Opus 4.5".into(),
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        base_url,
        reasoning: true,
        input: vec![Modality::Text, Modality::Image],
        cost: ModelCost {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
            tiers: None,
        },
        context_window: 200_000,
        max_tokens: 64_000,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

fn auth() -> AuthResult {
    let mut a = AuthResult::from_key("test-key-not-a-real-credential", "test");
    // Pin proxy resolution off for this request. `AnthropicMessagesApi::run` resolves the proxy
    // through `build_client_for_target(&url, &EnvAuthContext, auth.env.as_ref(), …)`
    // (`anthropic_messages.rs:189`), and the provider env overlay wins over the ambient process
    // env (`utils/node_http_proxy.rs:38-58`). Without this, a developer's `HTTP_PROXY` would send
    // the loopback request off-box.
    let mut env = ProviderEnv::new();
    env.insert("no_proxy".to_string(), "*".to_string());
    a.env = Some(env);
    a
}

/// A well-formed Anthropic SSE transcript whose `message_delta` carries `stop_reason`.
fn transcript(stop_reason_json: &str) -> String {
    format!(
        concat!(
            "event: message_start\n",
            "data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_sensitive\",\"usage\":{{\"input_tokens\":11,\"output_tokens\":0}}}}}}\n\n",
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n",
            "event: content_block_delta\n",
            "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"partial\"}}}}\n\n",
            "event: content_block_stop\n",
            "data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n",
            "event: message_delta\n",
            "data: {{\"type\":\"message_delta\",\"delta\":{},\"usage\":{{\"output_tokens\":4}}}}\n\n",
            "event: message_stop\n",
            "data: {{\"type\":\"message_stop\"}}\n\n",
        ),
        stop_reason_json
    )
}

/// Run one real Anthropic turn against a loopback server and return every emitted event.
async fn run_turn(stop_reason_json: &str) -> Vec<StreamEvent> {
    let url = spawn_sse_server(transcript(stop_reason_json));
    let api = AnthropicMessagesApi::new();
    let m = model(url);
    let ctx = Context {
        system_prompt: Some("be brief".to_string()),
        messages: vec![Message::User {
            content: vec![Content::text("hello")],
            timestamp: 0,
        }],
        tools: Vec::new(),
    };
    let (sink, mut rx) = channel(64);
    let task = tokio::spawn(async move {
        api.run(
            &m,
            &ctx,
            &auth(),
            &StreamOptions::default(),
            CancelToken::new(),
            sink,
        )
        .await;
    });
    let mut events = Vec::new();
    // Drain to completion — the terminal event is the last one, and the channel only closes once
    // `run` has dropped its sink.
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    task.await.expect("api task");
    events
}

/// The terminal `StreamEvent::Error`'s `error_message`, or a description of what came instead.
fn terminal_error_message(events: &[StreamEvent]) -> String {
    let err = events.iter().find_map(|e| match e {
        StreamEvent::Error { error, .. } => Some(error.clone()),
        _ => None,
    });
    let Some(msg) = err else {
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                StreamEvent::Start { .. } => "start",
                StreamEvent::Done { .. } => "done",
                StreamEvent::Error { .. } => "error",
                _ => "other",
            })
            .collect();
        panic!("no terminal StreamEvent::Error; got {kinds:?}");
    };
    assert_eq!(
        msg.stop_reason,
        StopReason::Error,
        "an error terminal must carry StopReason::Error"
    );
    msg.error_message
        .clone()
        .unwrap_or_else(|| "<error_message was None>".to_string())
}

// ---------------------------------------------------------------------------------- the finding --

/// pi v0.83.0 `mapStopReason` maps `sensitive` to
/// `{ stopReason: "error", errorMessage: "Provider stopped with: sensitive" }`. cyrup dropped the
/// message, so its terminal fell through to the `"An unknown error occurred"` fallback and a
/// safety-filter stop looked exactly like a dead socket.
#[tokio::test]
async fn sensitive_stop_reason_reaches_the_transcript_as_its_own_diagnostic() {
    let events = run_turn(r#"{"stop_reason":"sensitive"}"#).await;
    let message = terminal_error_message(&events);

    assert_ne!(
        message, "An unknown error occurred",
        "the `sensitive` arm dropped its diagnostic, so the terminal fell through to the generic \
         fallback — a content-policy stop is now indistinguishable from a transport failure"
    );
    assert_eq!(
        message, "Provider stopped with: sensitive",
        "pi v0.83.0 anthropic-messages.ts mapStopReason: \
         case \"sensitive\": return {{ stopReason: \"error\", \
         errorMessage: \"Provider stopped with: sensitive\" }}"
    );
}

// -------------------------------------------------------------------------------- mirror cases --

/// MIRROR (green before and after the fix). `refusal` is the sibling arm that always carried text;
/// it proves the loopback harness, the decoder wiring and `terminal_error_message` all work, so a
/// failure above is about the `sensitive` mapping alone.
#[tokio::test]
async fn refusal_carries_its_explanation() {
    let events = run_turn(
        r#"{"stop_reason":"refusal","stop_details":{"type":"refusal","explanation":"I can't help with that."}}"#,
    )
    .await;
    assert_eq!(
        terminal_error_message(&events),
        "I can't help with that.",
        "pi maps refusal to `stopDetails?.explanation || \"The model refused …\"`"
    );
}

/// MIRROR (green before and after the fix). The catch-all arm names the reason it could not handle
/// — the behaviour the `sensitive` arm was measurably *worse* than, since deleting `sensitive`
/// entirely would at least have produced `Unhandled stop reason: sensitive`.
#[tokio::test]
async fn unknown_stop_reason_names_itself() {
    let events = run_turn(r#"{"stop_reason":"brand_new_reason"}"#).await;
    assert_eq!(
        terminal_error_message(&events),
        "Unhandled stop reason: brand_new_reason",
        "pi's default arm throws `Unhandled stop reason: ${{reason}}`"
    );
}

/// MIRROR (green before and after the fix). A non-error stop must not be pushed down the error
/// path at all — this pins that the harness distinguishes a `Done` terminal from an `Error` one,
/// so the assertions above cannot pass by accident on a stream that never errored.
#[tokio::test]
async fn end_turn_is_not_an_error_terminal() {
    let events = run_turn(r#"{"stop_reason":"end_turn"}"#).await;
    let done = events.iter().find_map(|e| match e {
        StreamEvent::Done { message, .. } => Some(message.clone()),
        _ => None,
    });
    let msg = done.expect("done terminal");
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.error_message, None);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::Error { .. })),
        "end_turn must not emit an error terminal"
    );
}
