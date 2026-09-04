//! [`crate::RpcClient`] — the SEAM-017 port of Pi `modes/rpc/rpc-client.ts` @v0.83.0.
//!
//! Every case drives the real client over a real `tokio::io::duplex` pair against a *scripted host*
//! that speaks the same strict-LF JSONL Pi's `rpc-mode.ts` writes. Nothing here mocks the client's
//! internals: the correlation map, the listener list and the framing are exercised end to end, which
//! is the whole point of the item (an in-tree reader of the wire is what would have caught SEAM-011
//! and SEAM-053).
//!
//! The three tests named `*_mechanism_gap_*` pin the JS→Rust hazards the module documents rather
//! than any behaviour Pi could observe:
//! * a Rust future can be dropped at any `.await`, so both cleanups must be `Drop`, not statements
//!   on a success path;
//! * a re-entered handler in Rust can re-take a held `Mutex` and hang with no deadlock detection;
//! * host EOF must fail an in-flight request, because a Rust caller has no `exit` event to observe.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

use crate::{RpcClient, RpcClientError, event_type};

// ---------------------------------------------------------------------------------------------
// A scripted host — the other half of the protocol, spoken by hand
// ---------------------------------------------------------------------------------------------

/// The host end of a duplex pair: reads command lines the client wrote, writes response/event lines
/// back. Deliberately hand-rolled — if this used the client it would prove nothing about the wire.
struct ScriptedHost {
    lines: tokio::io::Lines<BufReader<DuplexStream>>,
    out: DuplexStream,
}

impl ScriptedHost {
    /// Read the next command line the client sent, parsed.
    async fn next_command(&mut self) -> Value {
        let line = self
            .lines
            .next_line()
            .await
            .expect("host read")
            .expect("client closed its writer before sending a command");
        serde_json::from_str(&line).expect("client wrote a non-JSON line")
    }

    /// Write one raw line (LF-framed, as `serializeJsonLine` does).
    async fn write_line(&mut self, value: &Value) {
        let line = format!("{value}\n");
        self.out
            .write_all(line.as_bytes())
            .await
            .expect("host write");
        self.out.flush().await.expect("host flush");
    }

    /// Write a raw, possibly-malformed line.
    async fn write_raw(&mut self, text: &str) {
        self.out
            .write_all(format!("{text}\n").as_bytes())
            .await
            .expect("host write");
        self.out.flush().await.expect("host flush");
    }

    /// Close ONLY the host's write half, so the client sees EOF on its reader while its own write
    /// half stays open — the shape of a child that has exited but whose stdin pipe the parent still
    /// holds. Dropping the whole host instead would race a write error against the EOF.
    fn close_output(&mut self) {
        let (dangling, _closed_peer) = tokio::io::duplex(1);
        self.out = dangling;
    }

    /// The success shape `rpc.rs`'s `RpcResponse::ok` emits.
    async fn respond_ok(&mut self, id: &Value, command: &str, data: Value) {
        self.write_line(&json!({
            "id": id, "type": "response", "command": command, "success": true, "data": data,
        }))
        .await;
    }
}

/// Wire a client to a scripted host over two duplex pipes.
fn connect() -> (RpcClient, ScriptedHost) {
    // client-writes → host-reads
    let (client_w, host_r) = tokio::io::duplex(64 * 1024);
    // host-writes → client-reads
    let (host_w, client_r) = tokio::io::duplex(64 * 1024);
    let client = RpcClient::attach(BufReader::new(client_r), client_w);
    let host = ScriptedHost {
        lines: BufReader::new(host_r).lines(),
        out: host_w,
    };
    (client, host)
}

// ---------------------------------------------------------------------------------------------
// Correlation
// ---------------------------------------------------------------------------------------------

/// Pi `send` builds `req_${++this.requestId}` (`rpc-client.ts:559`), so the FIRST id is `req_1`, not
/// `req_0`, and the response that carries it back resolves exactly that request.
#[tokio::test]
async fn first_request_id_is_req_1_and_the_reply_is_correlated_to_it() {
    let (client, mut host) = connect();

    let call = tokio::spawn(async move {
        let state = client.get_state().await.expect("get_state");
        (state, client)
    });

    let command = host.next_command().await;
    assert_eq!(command["type"], json!("get_state"));
    assert_eq!(command["id"], json!("req_1"));

    // A response for a DIFFERENT id must not settle this request — it is an event, per Pi's
    // `pendingRequests.has(data.id)` guard (`rpc-client.ts:512`).
    host.respond_ok(&json!("req_999"), "get_state", json!({"stray": true}))
        .await;
    host.respond_ok(&command["id"], "get_state", json!({"sessionId": "s1"}))
        .await;

    let (state, client) = call.await.expect("join");
    assert_eq!(state["sessionId"], json!("s1"));
    // The map is empty again — the pending entry was removed by the guard, not left behind.
    assert_eq!(client.pending_count(), 0);
}

/// The second request increments (`req_2`), and two in-flight requests answered OUT OF ORDER each
/// resolve their own caller — the property a hand-rolled `read_json_line` loop does not have.
#[tokio::test]
async fn concurrent_requests_resolve_out_of_order_by_id() {
    let (client, mut host) = connect();
    let client = Arc::new(client);

    let a = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.get_state().await }
    });
    let first = host.next_command().await;
    let b = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.get_session_stats().await }
    });
    let second = host.next_command().await;

    assert_eq!(first["id"], json!("req_1"));
    assert_eq!(second["id"], json!("req_2"));
    assert_eq!(second["type"], json!("get_session_stats"));

    // Answer the SECOND one first.
    host.respond_ok(
        &second["id"],
        "get_session_stats",
        json!({"which": "stats"}),
    )
    .await;
    host.respond_ok(&first["id"], "get_state", json!({"which": "state"}))
        .await;

    assert_eq!(
        b.await.expect("join b").expect("stats")["which"],
        json!("stats")
    );
    assert_eq!(
        a.await.expect("join a").expect("state")["which"],
        json!("state")
    );
}

/// Pi `getData` rethrows the response's own `error` string (`rpc-client.ts:591-594`), so the client's
/// error text is the HOST's text, unwrapped and unprefixed.
#[tokio::test]
async fn an_error_response_surfaces_the_hosts_error_string_verbatim() {
    let (client, mut host) = connect();

    let call = tokio::spawn(async move { client.set_model("acme", "nope").await });

    let command = host.next_command().await;
    assert_eq!(command["provider"], json!("acme"));
    assert_eq!(command["modelId"], json!("nope"));
    host.write_line(&json!({
        "id": command["id"], "type": "response", "command": "set_model",
        "success": false, "error": "Model not found: acme/nope",
    }))
    .await;

    let error = call.await.expect("join").expect_err("must be an error");
    assert!(matches!(error, RpcClientError::Command(_)));
    assert_eq!(error.to_string(), "Model not found: acme/nope");
}

// ---------------------------------------------------------------------------------------------
// Event dispatch
// ---------------------------------------------------------------------------------------------

/// Pi `handleLine`: anything that is not a correlated `response` goes to the listeners, and a line
/// that does not parse as JSON is swallowed (`rpc-client.ts:507-525`). A `response` whose id nobody
/// is waiting on falls through to the listeners too — that fall-through is Pi's, not an accident.
#[tokio::test]
async fn events_reach_listeners_garbage_is_ignored_and_an_orphan_response_is_an_event() {
    let (client, mut host) = connect();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let _sub = {
        let seen = Arc::clone(&seen);
        client.on_event(move |event| {
            let kind = event_type(event).unwrap_or("<none>").to_string();
            seen.lock().unwrap().push(kind);
        })
    };

    host.write_raw("this is not json").await;
    host.write_line(&json!({"type": "agent_start"})).await;
    host.write_line(&json!({
        "id": "req_404", "type": "response", "command": "get_state", "success": true, "data": {},
    }))
    .await;
    host.write_line(&json!({"type": "agent_settled"})).await;

    // Drive the reader until the terminal event lands rather than sleeping for it.
    for _ in 0..2_000 {
        if seen.lock().unwrap().iter().any(|k| k == "agent_settled") {
            break;
        }
        tokio::task::yield_now().await;
    }
    let seen = seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            "agent_start".to_string(),
            "response".to_string(),
            "agent_settled".to_string()
        ],
        "the non-JSON line must be dropped and the orphan response delivered as an event"
    );
}

/// `wait_for_idle` resolves on `agent_settled` and `collect_events` returns every event up to and
/// including it (Pi `rpc-client.ts:455-492`).
#[tokio::test]
async fn collect_events_returns_the_turn_up_to_and_including_agent_settled() {
    let (client, mut host) = connect();
    let client = Arc::new(client);

    let collecting = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.collect_events(Duration::from_secs(10)).await }
    });
    // Let the collector's listener register before the host writes anything. Bounded, so a
    // regression that never registers fails with this message instead of hanging the suite.
    let mut armed = false;
    for _ in 0..2_000 {
        if client.listener_count() == 1 {
            armed = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(armed, "collect_events never registered its listener");

    host.write_line(&json!({"type": "agent_start"})).await;
    host.write_line(&json!({"type": "message_end"})).await;
    host.write_line(&json!({"type": "agent_settled"})).await;
    host.write_line(&json!({"type": "agent_start"})).await;

    let events = collecting.await.expect("join").expect("collect");
    let kinds: Vec<&str> = events.iter().filter_map(event_type).collect();
    assert_eq!(kinds, vec!["agent_start", "message_end", "agent_settled"]);
}

/// Pi arms the collector BEFORE sending (`promptAndWait`, `rpc-client.ts:497-501`:
/// `const eventsPromise = this.collectEvents(timeout); await this.prompt(...)`). A host that answers
/// the prompt and settles the turn in the same breath would otherwise race the subscription — with
/// the arming inverted this test times out instead of returning.
#[tokio::test]
async fn prompt_and_wait_arms_the_collector_before_the_prompt_is_written() {
    let (client, mut host) = connect();

    let call = tokio::spawn(async move {
        client
            .prompt_and_wait("hello", None, Duration::from_secs(10))
            .await
    });

    let command = host.next_command().await;
    assert_eq!(command["type"], json!("prompt"));
    assert_eq!(command["message"], json!("hello"));
    // Pi passes `images` as an optional property, so an absent image list is an absent KEY, never
    // `null` (SEAM-053's rule, applied on the client's write side).
    assert!(
        command.get("images").is_none(),
        "an absent image list must omit the key, not send null: {command}"
    );

    host.respond_ok(&command["id"], "prompt", Value::Null).await;
    host.write_line(&json!({"type": "agent_start"})).await;
    host.write_line(&json!({"type": "agent_settled"})).await;

    let events = call.await.expect("join").expect("prompt_and_wait");
    let kinds: Vec<&str> = events.iter().filter_map(event_type).collect();
    assert_eq!(kinds, vec!["agent_start", "agent_settled"]);
}

// ---------------------------------------------------------------------------------------------
// Typed unwrapping
// ---------------------------------------------------------------------------------------------

/// Pi's client narrows `{ models }` to its own structural `ModelInfo` (`rpc-client.ts:42-47`,
/// `:263-266`) — the host sends whole model objects (`rpc-mode.ts:485-487`), so the extra keys must
/// be ignored, not rejected.
#[tokio::test]
async fn get_available_models_unwraps_the_envelope_and_ignores_extra_model_keys() {
    let (client, mut host) = connect();

    let call = tokio::spawn(async move { client.get_available_models().await });

    let command = host.next_command().await;
    host.respond_ok(
        &command["id"],
        "get_available_models",
        json!({"models": [{
            "id": "m1", "provider": "acme", "contextWindow": 200000, "reasoning": true,
            "name": "M One", "baseUrl": "https://acme.test", "maxTokens": 8192,
        }]}),
    )
    .await;

    let models = call.await.expect("join").expect("models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "m1");
    assert_eq!(models[0].provider, "acme");
    assert_eq!(models[0].context_window, 200_000);
    assert!(models[0].reasoning);
}

/// The `{ cancelled }` shape every session-replacing verb answers with
/// (`rpc-client.ts:227`/`:369`/`:387`; host `rpc.rs` `json!({"cancelled": …})`).
#[tokio::test]
async fn new_session_reports_an_extension_cancellation() {
    let (client, mut host) = connect();
    let client = Arc::new(client);

    let call = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.new_session(Some("/tmp/parent.jsonl")).await }
    });
    let command = host.next_command().await;
    assert_eq!(command["parentSession"], json!("/tmp/parent.jsonl"));
    host.respond_ok(&command["id"], "new_session", json!({"cancelled": true}))
        .await;
    assert!(call.await.expect("join").expect("new_session"));

    // No parent ⇒ absent key, never `null` (Pi spreads an `undefined`, SEAM-053).
    let call = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.new_session(None).await }
    });
    let command = host.next_command().await;
    assert!(
        command.get("parentSession").is_none(),
        "an absent parentSession must omit the key: {command}"
    );
    host.respond_ok(&command["id"], "new_session", json!({"cancelled": false}))
        .await;
    assert!(!call.await.expect("join").expect("new_session"));
}

// ---------------------------------------------------------------------------------------------
// Mechanism gaps — the JS guarantees Rust does not give
// ---------------------------------------------------------------------------------------------

/// **Mechanism gap 1a.** Pi unsubscribes from the RESOLVE path of `waitForIdle`
/// (`rpc-client.ts:465`), which is exhaustive in JS because an `async` function always settles. A
/// Rust future can be dropped at any `.await`, so the unsubscribe must be `Drop`.
///
/// The presence assertion comes first, deliberately: without it "0 listeners after the drop" passes
/// even if the listener never registered at all.
#[tokio::test]
async fn dropping_a_wait_for_idle_future_unsubscribes_mechanism_gap_1a() {
    let (client, _host) = connect();
    assert_eq!(client.listener_count(), 0);

    {
        let mut waiting = std::pin::pin!(client.wait_for_idle(Duration::from_secs(30)));
        let polled = futures::poll!(waiting.as_mut());
        assert!(polled.is_pending(), "nothing has settled the turn yet");
        // PRESENCE: the listener is really registered while the future is alive.
        assert_eq!(client.listener_count(), 1);
    }

    // ABSENCE: dropping the future — never awaiting it to completion — removed the listener.
    assert_eq!(
        client.listener_count(),
        0,
        "a cancelled wait_for_idle leaked its listener; the unsubscribe is on a success path, not in Drop"
    );
}

/// **Mechanism gap 1b.** The same argument for `pendingRequests`: Pi deletes the entry on the
/// resolve, reject and timeout paths (`rpc-client.ts:514`, `:564`, `:584`) and cannot be abandoned;
/// a dropped `send` future here must still remove it, or the map grows for the process's lifetime
/// and `rejectPendingRequests` iterates corpses.
#[tokio::test]
async fn dropping_a_send_future_removes_its_pending_entry_mechanism_gap_1b() {
    let (client, mut host) = connect();
    assert_eq!(client.pending_count(), 0);

    {
        let mut call = std::pin::pin!(client.get_state());
        let mut registered = false;
        for _ in 0..100 {
            let polled = futures::poll!(call.as_mut());
            assert!(polled.is_pending(), "no response has been written yet");
            if client.pending_count() == 1 {
                registered = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        // PRESENCE: the request really is registered, and really was written.
        assert!(registered, "the request never entered the correlation map");
        let command = host.next_command().await;
        assert_eq!(command["type"], json!("get_state"));
    }

    assert_eq!(
        client.pending_count(),
        0,
        "an abandoned request stayed in the correlation map; the delete is on a success path, not in Drop"
    );
}

/// **Mechanism gap 2.** Pi's `handleLine` iterates `eventListeners` directly
/// (`rpc-client.ts:520-522`); JS has no locks, so a listener that unsubscribes *during* dispatch is
/// an ordinary nested call. Holding the listener `Mutex` across the callback would let that call
/// re-take a held lock and hang with no deadlock detection — so the dispatcher snapshots and
/// releases first.
///
/// A deadlock would park the reader task, so the assertion is bounded: the test fails loudly instead
/// of hanging the suite.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsubscribing_from_inside_a_listener_does_not_deadlock_mechanism_gap_2() {
    let (client, mut host) = connect();

    let fired = Arc::new(AtomicUsize::new(0));
    // A victim subscription the OTHER listener will drop while dispatch is in progress.
    let victim = {
        let fired = Arc::clone(&fired);
        client.on_event(move |_| {
            fired.fetch_add(1, Ordering::SeqCst);
        })
    };
    let slot: Arc<Mutex<Option<crate::EventSubscription>>> = Arc::new(Mutex::new(Some(victim)));
    let done = Arc::new(AtomicUsize::new(0));
    let _killer = {
        let slot = Arc::clone(&slot);
        let done = Arc::clone(&done);
        client.on_event(move |_| {
            // Dropping the subscription re-enters the listener list's lock from inside dispatch.
            drop(slot.lock().unwrap().take());
            done.fetch_add(1, Ordering::SeqCst);
        })
    };
    assert_eq!(client.listener_count(), 2);

    host.write_line(&json!({"type": "agent_start"})).await;

    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        while done.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        observed.is_ok(),
        "the reader task never finished dispatching — the listener lock was held across the callback"
    );
    // The victim ran for THIS event (it was in the snapshot) and is gone for the next one.
    assert_eq!(fired.load(Ordering::SeqCst), 1);
    assert_eq!(client.listener_count(), 1);
}

/// **Mechanism gap 3.** Pi learns the child is gone from the `exit` event and calls
/// `rejectPendingRequests` (`rpc-client.ts:106-111`, `:532-537`). An attached client has no such
/// event, only EOF on the read half — so EOF must fail every in-flight request rather than let it
/// burn the full 30 s `REQUEST_TIMEOUT_MS`.
#[tokio::test]
async fn host_eof_fails_the_in_flight_request_instead_of_waiting_out_the_timeout() {
    let (client, mut host) = connect();

    let call = tokio::spawn(async move { client.get_state().await });
    let command = host.next_command().await;
    assert_eq!(command["type"], json!("get_state"));

    // The host dies without answering.
    drop(host);

    let error = tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .expect("EOF must fail the request promptly, not after REQUEST_TIMEOUT_MS")
        .expect("join")
        .expect_err("must be an error");
    assert!(
        matches!(error, RpcClientError::ProcessExited { .. }),
        "unexpected error: {error}"
    );
    assert!(
        error
            .to_string()
            .starts_with("Agent process exited (code=null signal=null).")
    );
}

/// Pi's `send` re-throws the latched `exitError` BEFORE touching stdin (`rpc-client.ts:545-547`), so
/// a request issued after the host is gone fails immediately and never registers a pending entry.
#[tokio::test]
async fn a_request_after_the_host_is_gone_is_pre_empted_by_the_latched_exit_error() {
    let (client, mut host) = connect();
    // The host exits. Only its write half closes, so the client's own stdin stays writable and the
    // ONLY thing that can fail this request is the EOF-driven rejection.
    host.close_output();

    let first = tokio::time::timeout(Duration::from_secs(5), client.get_state())
        .await
        .expect("EOF must fail the request promptly")
        .expect_err("must be an error");
    assert!(
        matches!(first, RpcClientError::ProcessExited { .. }),
        "unexpected: {first}"
    );

    // The SECOND request is pre-empted by the latched error rather than written and awaited.
    let second = tokio::time::timeout(Duration::from_millis(500), client.get_state())
        .await
        .expect("the latched exit error must pre-empt the write, not wait for a response")
        .expect_err("must be an error");
    assert!(
        matches!(second, RpcClientError::ProcessExited { .. }),
        "unexpected: {second}"
    );
    assert_eq!(
        client.pending_count(),
        0,
        "a pre-empted request must never enter the correlation map"
    );
}

// ---------------------------------------------------------------------------------------------
// Error strings
// ---------------------------------------------------------------------------------------------

/// The client's error text is part of its contract — Pi's embedders match on these strings. Pinned
/// against `rpc-client.ts:75`, `:459`, `:480`, `:529`, `:543`, `:554`, `:565`.
#[test]
fn error_strings_are_pis_verbatim() {
    assert_eq!(
        RpcClientError::AlreadyStarted.to_string(),
        "Client already started"
    );
    assert_eq!(RpcClientError::NotStarted.to_string(), "Client not started");
    assert_eq!(
        RpcClientError::ProcessExited {
            code: "1".into(),
            signal: "null".into(),
            stderr: "boom".into(),
        }
        .to_string(),
        "Agent process exited (code=1 signal=null). Stderr: boom"
    );
    assert_eq!(
        RpcClientError::ProcessError {
            message: "EPIPE".into(),
            stderr: "s".into()
        }
        .to_string(),
        "Agent process error: EPIPE. Stderr: s"
    );
    assert_eq!(
        RpcClientError::StdinNotWritable { stderr: "s".into() }.to_string(),
        "Agent process stdin is not writable. Stderr: s"
    );
    assert_eq!(
        RpcClientError::RequestTimeout {
            command: "prompt".into(),
            stderr: "s".into()
        }
        .to_string(),
        "Timeout waiting for response to prompt. Stderr: s"
    );
    assert_eq!(
        RpcClientError::IdleTimeout { stderr: "s".into() }.to_string(),
        "Timeout waiting for agent to become idle. Stderr: s"
    );
    assert_eq!(
        RpcClientError::CollectTimeout { stderr: "s".into() }.to_string(),
        "Timeout collecting events. Stderr: s"
    );
    // Pi's two default timeouts, as literals (`rpc-client.ts:455`, `:566`).
    assert_eq!(crate::REQUEST_TIMEOUT_MS, 30_000);
    assert_eq!(crate::DEFAULT_IDLE_TIMEOUT_MS, 60_000);
}

/// `RpcResponse` is now read as well as written (the SEAM-017 `Deserialize`): a round trip must
/// survive both directions, including the `id`-omitted and `error` shapes.
#[test]
fn rpc_response_round_trips_through_the_new_deserialize() {
    let wire = json!({
        "id": 7, "type": "response", "command": "get_state", "success": true, "data": {"a": 1},
    });
    let parsed: crate::RpcResponse = serde_json::from_value(wire.clone()).expect("parse");
    assert_eq!(parsed.id, Some(json!(7)));
    assert_eq!(parsed.kind, "response");
    assert_eq!(parsed.command, "get_state");
    assert!(parsed.success);
    assert_eq!(parsed.data, Some(json!({"a": 1})));
    assert_eq!(parsed.error, None);
    assert_eq!(serde_json::to_value(&parsed).expect("serialize"), wire);

    let failed: crate::RpcResponse = serde_json::from_value(json!({
        "type": "response", "command": "fork", "success": false, "error": "nope",
    }))
    .expect("parse");
    assert_eq!(failed.id, None);
    assert!(!failed.success);
    assert_eq!(failed.error.as_deref(), Some("nope"));
}
