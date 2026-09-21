//! [`HerdrClient`]'s own contract, independent of which method is being called: it holds no
//! connection, it correlates every answer, and every call is bounded.

use std::time::{Duration, Instant};

use super::fake_server::{FakeHerdr, Reply, request_method, result_for};
use crate::client::HerdrClient;
use crate::env::{HERDR_ENV, HERDR_PANE_ID, HERDR_SOCKET_PATH};
use crate::error::{HerdrError, Unavailable};
use crate::schema::common::{EmptyParams, PaneTarget};
use crate::schema::{Method, ResponseResult};
use crate::transport::DEFAULT_TIMEOUT;

/// AUG §5.2's deadline row, and the reason it exists: **herdr imposes none**.
/// `dispatch_to_app` is a `recv()` with a `None` timeout
/// (`tmp/herdr/src/api/server.rs:911-913`), so a wedged UI never answers and never closes. 15 s
/// matches pi's own `SocketRpcClient` (`src/runs/shared/herdr-connection.ts:78` @v0.68.0) and is
/// deliberately longer than herdr's 5 s `APP_RESPONSE_TIMEOUT` (`server.rs:29`), so a herdr that
/// is merely slow gets to answer — including with its own `timeout` code.
#[test]
fn the_default_request_deadline_is_fifteen_seconds() {
    assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(15));
    assert_eq!(
        HerdrClient::new("/nonexistent/herdr.sock".into()).timeout(),
        DEFAULT_TIMEOUT,
        "a client that named no deadline still has one"
    );
    assert_eq!(
        HerdrClient::new("/nonexistent/herdr.sock".into())
            .with_timeout(Duration::from_secs(3))
            .timeout(),
        Duration::from_secs(3)
    );
}

/// Constructing a client must touch nothing: herdr serves one request per connection
/// (`tmp/herdr/src/api/server.rs:156-317`), so there is no connection worth holding, and
/// [`HerdrClient::new`] is a path and a deadline (`tmp/herdr/src/api/client.rs:33-44` is the same
/// shape).
///
/// The fake is **live** while this runs, so "no connection was opened" is an observation, not an
/// absence of evidence.
#[tokio::test]
async fn constructing_a_client_opens_nothing() {
    let fake = FakeHerdr::start_with(|request| result_for(request, r#"{"type":"ok"}"#));

    let client = HerdrClient::new(fake.path().to_path_buf()).with_timeout(Duration::from_secs(2));
    let clone = client.clone();
    let _for_pane = HerdrClient::new(fake.path().to_path_buf());

    assert_eq!(client.socket(), fake.path());
    assert_eq!(clone.socket(), fake.path());
    assert!(
        fake.received().is_empty(),
        "no call was made, so nothing may have been sent: {:?}",
        fake.received()
    );

    // And the very same client does reach the socket once something asks it to.
    client.pane_close("w1:p1").await.expect("acknowledged");
    assert_eq!(fake.received().len(), 1);
}

/// [`HerdrClient::for_pane`] reads the socket out of the discovered pane and nothing else —
/// the same path `HERDR_SOCKET_PATH` named. A client built this way is the production shape:
/// `HerdrPane::discover(…)` then `HerdrClient::for_pane(&pane)`.
#[tokio::test]
async fn for_pane_talks_to_the_panes_own_socket() {
    let fake = FakeHerdr::start_with(|request| {
        result_for(
            request,
            r#"{"type":"pane_info","pane":{"pane_id":"w1:p1","terminal_id":"t1",
               "workspace_id":"w1","tab_id":"w1:t1","focused":true,"agent_status":"idle",
               "revision":1}}"#,
        )
    });

    let env: std::collections::HashMap<String, String> = [
        (HERDR_ENV.to_owned(), "1".to_owned()),
        (HERDR_PANE_ID.to_owned(), "w1:p1".to_owned()),
        (
            HERDR_SOCKET_PATH.to_owned(),
            fake.path().display().to_string(),
        ),
    ]
    .into_iter()
    .collect();

    let pane = crate::HerdrPane::discover(&env).expect("inside a pane");
    let client = HerdrClient::for_pane(&pane).with_timeout(Duration::from_secs(2));
    assert_eq!(client.socket(), fake.path());

    let info = client
        .pane_get(pane.pane_id())
        .await
        .expect("the pane's own socket answers");
    assert_eq!(info.pane_id, "w1:p1");
    assert_eq!(fake.received().len(), 1);
}

/// AUG §8.5, at the [`HerdrClient`] level rather than the transport's.
///
/// One request per connection means a mismatched `id` cannot be a pipelining artefact: the payload
/// describes something other than what was asked. Accepting it here would hand a caller **another
/// pane's record** as its own, which is the single worst failure this client can have, because it
/// is silent and it looks like data. pi drops it too
/// (`src/runs/shared/herdr-connection.ts:86` @v0.68.0).
#[tokio::test]
async fn a_typed_call_refuses_a_mismatched_id() {
    let fake = FakeHerdr::start(Reply::Line(super::fake_server::compact(
        r#"{"id":"someone_else","result":{"type":"pane_info","pane":{"pane_id":"w9:p9",
           "terminal_id":"t9","workspace_id":"w9","tab_id":"w9:t9","focused":false,
           "agent_status":"idle","revision":1}}}"#,
    )));

    let failure = HerdrClient::new(fake.path().to_path_buf())
        .with_timeout(Duration::from_secs(2))
        .pane_get("w1:p1")
        .await
        .expect_err("another pane's record is not this call's answer");

    match failure {
        HerdrError::IdMismatch { method, sent, got } => {
            assert_eq!(method, "pane.get");
            assert_eq!(got, "someone_else");
            assert!(
                sent.starts_with("cyrup:pane.get:"),
                "the sent id names its method so a log line is readable: {sent}"
            );
        }
        other => panic!("expected IdMismatch, got {other:?}"),
    }
}

/// Every call carries a distinct `id`, because [`HerdrError::IdMismatch`] can only mean something
/// if two calls cannot share one. herdr places no constraint on the shape — its own CLI sends the
/// constant `"cli:request"` (`tmp/herdr/src/cli.rs:757`) — so this is cyrup's own discipline, not
/// a protocol requirement.
#[tokio::test]
async fn every_call_carries_its_own_correlation_id() {
    let fake = FakeHerdr::start_with(|request| result_for(request, r#"{"type":"ok"}"#));
    let client = HerdrClient::new(fake.path().to_path_buf()).with_timeout(Duration::from_secs(2));

    for _ in 0..4_u8 {
        client.pane_close("w1:p1").await.expect("acknowledged");
    }

    let ids: std::collections::BTreeSet<String> = fake
        .received()
        .iter()
        .map(|line| super::fake_server::request_id(line))
        .collect();
    assert_eq!(ids.len(), 4, "four calls, four distinct ids: {ids:?}");
    assert!(
        ids.iter().all(|id| id.starts_with("cyrup:pane.close:")),
        "the id names its method: {ids:?}"
    );
}

/// The per-call deadline is the client's, and it is honoured on the wire — not just stored.
///
/// The fake accepts and reads and then never writes, which is exactly what a herdr with a wedged
/// UI does, and which has **no** server-side rescue.
#[tokio::test]
async fn a_client_deadline_bounds_every_call() {
    let fake = FakeHerdr::start(Reply::Silence);
    let client =
        HerdrClient::new(fake.path().to_path_buf()).with_timeout(Duration::from_millis(150));

    let started = Instant::now();
    let failure = client
        .session_snapshot()
        .await
        .expect_err("a server that never answers must not hang the caller");
    let elapsed = started.elapsed();

    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
    match failure {
        HerdrError::Timeout { method, timeout } => {
            assert_eq!(method, "session.snapshot");
            assert_eq!(
                timeout,
                Duration::from_millis(150),
                "the deadline reported is the one the caller set"
            );
        }
        other => panic!("expected Timeout, got {other:?}"),
    }
}

/// A socket that is not there is [`Unavailable::NoSocket`] **carrying the path**, on the first
/// call and on every typed method — not a bare io error, and never an `Ok`.
///
/// "herdr is not running" is not actionable; "nothing is listening on
/// /home/u/.config/herdr/herdr.sock" is.
#[tokio::test]
async fn a_dead_socket_names_the_path_on_a_typed_call() {
    let fake = FakeHerdr::start(Reply::Close);
    let dead = fake.dead_path();
    let client = HerdrClient::new(dead.clone()).with_timeout(Duration::from_secs(2));

    let failure = client
        .agent_list()
        .await
        .expect_err("nothing is listening there");

    match failure.unavailable() {
        Some(Unavailable::NoSocket { path, .. }) => assert_eq!(path, &dead),
        other => panic!("expected NoSocket carrying the path, got {other:?}"),
    }
    assert!(
        failure.to_string().contains(&dead.display().to_string()),
        "the path belongs in the sentence: {failure}"
    );
}

/// [`HerdrClient::call`] is the escape hatch for a method with no typed wrapper, and it is a raw
/// [`ResponseResult`] on purpose: it hands back exactly what herdr said, including
/// [`ResponseResult::Unrecognised`] for a `type` newer than this client, so the caller decides.
///
/// It is **not** a way around the typed accessors' refusal to fabricate: `Unrecognised` is a
/// value here and an error the moment anything asks it to be a record.
#[tokio::test]
async fn call_hands_back_the_raw_result_including_an_unrecognised_one() {
    let fake = FakeHerdr::start_with(|request| {
        let result = if request_method(request) == "agent.list" {
            r#"{"type":"agent_list","agents":[]}"#
        } else {
            r#"{"type":"a_type_from_a_newer_herdr","payload":{"anything":1}}"#
        };
        result_for(request, result)
    });
    let client = HerdrClient::new(fake.path().to_path_buf()).with_timeout(Duration::from_secs(2));

    let known = client
        .call(Method::AgentList(EmptyParams {}))
        .await
        .expect("a known result");
    assert_eq!(known.type_name(), "agent_list");
    assert_eq!(known.agent_list("agent.list").expect("decodes").len(), 0);

    let newer = client
        .call(Method::PaneGet(PaneTarget::new("w1:p1")))
        .await
        .expect("an unrecognised result decodes rather than failing the line");
    assert_eq!(newer, ResponseResult::Unrecognised);
    match newer.pane("pane.get") {
        Err(HerdrError::UnexpectedResult { want, got, .. }) => {
            assert_eq!(want, "pane_info");
            assert_eq!(got, "an unrecognised result type");
        }
        other => panic!("an unrecognised result must never become a record: {other:?}"),
    }
}

/// [`HerdrClient::call_for`] overrides the client's deadline for one call, which is what
/// [`HerdrClient::pane_wait_for_output`] is built on.
#[tokio::test]
async fn call_for_overrides_the_clients_deadline_for_one_call() {
    let fake = FakeHerdr::start(Reply::Silence);
    let client = HerdrClient::new(fake.path().to_path_buf()).with_timeout(Duration::from_secs(30));

    let started = Instant::now();
    let failure = client
        .call_for(
            Method::SessionSnapshot(EmptyParams {}),
            Duration::from_millis(120),
        )
        .await
        .expect_err("the per-call deadline applies");
    assert!(started.elapsed() < Duration::from_secs(2));

    match failure {
        HerdrError::Timeout { timeout, .. } => assert_eq!(timeout, Duration::from_millis(120)),
        other => panic!("expected Timeout, got {other:?}"),
    }
}

/// A `ping` through [`HerdrClient`] is the same `ping` [`crate::probe::ping`] sends, on **this
/// client's socket** and **this client's deadline** — so a consumer that already holds a client
/// does not need a second entry point to check liveness, and does not silently get a different
/// one.
///
/// Both halves are asserted against a live fake, because both are ways to have a `ping` that
/// looks like it works: one that probes the ambient socket ladder instead of the pane's own
/// socket, and one that carries the 15 s default into a caller who asked for 150 ms.
#[tokio::test]
async fn ping_through_the_client_uses_the_clients_socket_and_deadline() {
    let fake = FakeHerdr::start_with(|request| {
        super::fake_server::pong_for(request, Some(r#"{"live_handoff":true}"#))
    });

    let pong = HerdrClient::new(fake.path().to_path_buf())
        .with_timeout(Duration::from_secs(2))
        .ping()
        .await
        .expect("a pong");
    assert_eq!(pong.version(), "0.9.1");
    assert_eq!(pong.protocol(), 22);

    let received = fake.received();
    let line = received.first().expect("one line on THIS client's socket");
    assert_eq!(request_method(line), "ping");

    let silent = FakeHerdr::start(Reply::Silence);
    let started = Instant::now();
    let failure = HerdrClient::new(silent.path().to_path_buf())
        .with_timeout(Duration::from_millis(150))
        .ping()
        .await
        .expect_err("a server that accepts and never answers must not hang a liveness probe");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the client's own deadline must reach the probe"
    );
    match failure {
        HerdrError::Timeout { method, timeout } => {
            assert_eq!(method, "ping");
            assert_eq!(timeout, Duration::from_millis(150));
        }
        other => panic!("expected Timeout, got {other:?}"),
    }
}
