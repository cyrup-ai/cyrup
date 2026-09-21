//! The `ping` round trip against a fake herdr, and every way an answer can be refused.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::fake_server::{FakeHerdr, Reply, error_for, pong_for, request_id};
use crate::env::{HERDR_ENV, HERDR_PANE_ID, HERDR_SOCKET_PATH};
use crate::error::{ApiErrorCode, HerdrError, Unavailable};
use crate::probe::{Capability, ping, ping_current_pane, ping_for};

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// AUG §8.1. The fake asserts the bytes it received, because the thing most easily broken here is
/// invisible from the client side: **`params` is mandatory on every method, `ping` included.**
/// `Method` is adjacently tagged (`tmp/herdr/src/api/schema.rs:41`) with a newtype variant per
/// method, so the published schema's `ping` variant requires `["method","params"]` and a real herdr
/// answers `invalid_request` (`tmp/herdr/src/api/server.rs:198`) to a line without it.
#[tokio::test]
async fn ping_round_trips_and_exposes_version_protocol_capabilities() {
    let fake = FakeHerdr::start_with(|request| {
        pong_for(
            request,
            Some(
                r#"{"live_handoff":true,"detached_server_daemon":false,"endpoint_protocol_generation":3,"surface_interest":true,"health_check":false}"#,
            ),
        )
    });

    let pong = ping(fake.path()).await.expect("the fake answers a pong");

    let received = fake.received();
    assert_eq!(
        received.len(),
        1,
        "one request per connection: {received:?}"
    );
    let line = received.first().expect("one line");
    let id = request_id(line);
    assert_eq!(
        line,
        &format!(r#"{{"id":"{id}","method":"ping","params":{{}}}}"#),
        "the request must carry `params` — a real herdr rejects the line without it"
    );

    assert_eq!(pong.version(), "0.9.1");
    assert_eq!(pong.protocol(), 22);
    assert!(pong.supports(Capability::LiveHandoff));
    assert!(pong.supports(Capability::SurfaceInterest));
    assert!(!pong.supports(Capability::DetachedServerDaemon));
    assert!(!pong.supports(Capability::HealthCheck));
    assert_eq!(pong.endpoint_protocol_generation(), Some(3));
}

/// A server old enough to send no capability block must answer `false` for every flag — never
/// `true`, and never a parse failure. `capabilities` is `#[serde(default)] Option<…>`
/// (`tmp/herdr/src/api/schema/response.rs:47-49`).
#[tokio::test]
async fn an_absent_capability_block_declares_nothing() {
    let fake = FakeHerdr::start_with(|request| {
        Reply::Line(format!(
            r#"{{"id":"{}","result":{{"type":"pong","version":"0.7.5","protocol":18}}}}"#,
            request_id(request)
        ))
    });

    let pong = ping(fake.path())
        .await
        .expect("a pong without capabilities");

    assert_eq!(pong.version(), "0.7.5");
    assert_eq!(pong.protocol(), 18, "protocol is recorded, never gated on");
    assert!(pong.capabilities().is_none());
    for capability in [
        Capability::LiveHandoff,
        Capability::DetachedServerDaemon,
        Capability::SurfaceInterest,
        Capability::HealthCheck,
    ] {
        assert!(
            !pong.supports(capability),
            "undeclared must read as off: {capability:?}"
        );
    }
}

/// AUG §8.2. A `result.type` this client does not name decodes to
/// `ResponseResult::Unrecognised` — which is how `socket-api.mdx:959` ("clients should ignore
/// unknown fields") is honoured — but the typed accessor must still refuse it. herdr's own client
/// does the same (`tmp/herdr/src/api/client.rs:119`).
#[tokio::test]
async fn an_unknown_result_type_is_not_a_success() {
    let fake = FakeHerdr::start_with(|request| {
        Reply::Line(format!(
            r#"{{"id":"{}","result":{{"type":"martian","moons":2}}}}"#,
            request_id(request)
        ))
    });

    let error = ping(fake.path()).await.unwrap_err();

    assert!(
        matches!(
            &error,
            HerdrError::UnexpectedResult {
                method: "ping",
                want: "pong",
                ..
            }
        ),
        "an unrecognised result must be an error, not a defaulted success: {error:?}"
    );
}

/// AUG §8.3. herdr's code AND its message, byte for byte. pi folds a ~60-code space onto five
/// names (`src/inspectors/herdr/client.ts:35-41` @v0.68.0); this client does not.
#[tokio::test]
async fn an_error_envelope_carries_herdrs_own_code_and_message() {
    let fake = FakeHerdr::start_with(|request| {
        error_for(request, "pane_not_found", "pane w1:p9 not found")
    });

    let error = ping(fake.path()).await.unwrap_err();

    let HerdrError::Api { method, source } = &error else {
        panic!("expected an api error, got {error:?}");
    };
    assert_eq!(*method, "ping");
    assert_eq!(source.code, ApiErrorCode::PaneNotFound);
    assert_eq!(source.message, "pane w1:p9 not found");
    assert!(
        error.to_string().contains("pane w1:p9 not found"),
        "herdr's own sentence must survive Display: {error}"
    );
}

/// AUG §8.4. `code` is a bare `String` on the wire (`schema/response.rs:35-39`), so a herdr upgrade
/// that invents a code is a value — carrying that exact spelling — not a parse failure and not a
/// placeholder.
#[tokio::test]
async fn an_unknown_error_code_survives_as_other() {
    let fake = FakeHerdr::start_with(|request| {
        error_for(request, "brand_new_code", "something new went wrong")
    });

    let error = ping(fake.path()).await.unwrap_err();

    let HerdrError::Api { source, .. } = &error else {
        panic!("expected an api error, got {error:?}");
    };
    assert_eq!(
        source.code,
        ApiErrorCode::Other("brand_new_code".to_owned())
    );
    assert_eq!(source.code.as_str(), "brand_new_code");
    assert_eq!(source.message, "something new went wrong");
}

/// `invalid_request` is the one code that means *this herdr build does not have that method*: an
/// unknown method name fails `Request` deserialisation (`tmp/herdr/src/api/server.rs:177-204`).
/// A caller turns that one feature off — never the whole client.
#[tokio::test]
async fn invalid_request_is_the_one_code_that_means_unsupported_method() {
    let fake = FakeHerdr::start_with(|request| {
        error_for(
            request,
            "invalid_request",
            "invalid request: unknown variant `ping`",
        )
    });
    let unsupported = ping(fake.path()).await.unwrap_err();
    assert!(unsupported.is_unsupported_method());

    let busy = FakeHerdr::start_with(|request| error_for(request, "ui_busy", "the ui is busy"));
    let busy = ping(busy.path()).await.unwrap_err();
    assert!(
        !busy.is_unsupported_method(),
        "a busy UI is not a missing method"
    );
}

/// AUG §8.5. One request per connection means a mismatched `id` cannot be a pipelining artefact, so
/// the payload describes something other than what was asked. Accepting it would hand a caller
/// another pane's data as its own.
#[tokio::test]
async fn a_mismatched_response_id_is_refused() {
    let fake = FakeHerdr::start(Reply::Line(
        r#"{"id":"someone_else","result":{"type":"pong","version":"0.9.1","protocol":22}}"#
            .to_owned(),
    ));

    let error = ping(fake.path()).await.unwrap_err();

    let HerdrError::IdMismatch { method, sent, got } = &error else {
        panic!("expected an id mismatch, got {error:?}");
    };
    assert_eq!(*method, "ping");
    assert_eq!(got, "someone_else");
    assert_ne!(sent, "someone_else");
}

/// AUG §8.10. herdr writes `{"id":"","error":{"code":"invalid_request",…}}` when the line did not
/// deserialise and it could not recover a correlation id (`tmp/herdr/src/api/server.rs:180-201`).
/// An error envelope is fatal **whatever its id** — requiring a match first would make this shape
/// invisible and the call would sit until its deadline.
#[tokio::test]
async fn an_uncorrelated_invalid_request_is_fatal_not_a_wait() {
    let fake = FakeHerdr::start(Reply::Line(
        r#"{"id":"","error":{"code":"invalid_request","message":"invalid request: expected value"}}"#
            .to_owned(),
    ));

    let started = Instant::now();
    let error = ping_for(fake.path(), Duration::from_secs(30))
        .await
        .unwrap_err();

    assert!(
        started.elapsed() < Duration::from_secs(2),
        "an uncorrelated error must end the call at once, not wait out the deadline"
    );
    let HerdrError::Api { source, .. } = &error else {
        panic!("expected an api error, got {error:?}");
    };
    assert_eq!(source.code, ApiErrorCode::InvalidRequest);
}

/// AUG §8.11. There is no herdr-side deadline on a plain dispatch —
/// `tmp/herdr/src/api/server.rs:911-913` is a `recv()` with `None` timeout — so without this
/// client's own deadline a busy UI is a hang, not an error.
#[tokio::test]
async fn a_server_that_never_answers_times_out() {
    let fake = FakeHerdr::start(Reply::Silence);

    let started = Instant::now();
    let error = ping_for(fake.path(), Duration::from_millis(200))
        .await
        .unwrap_err();
    let elapsed = started.elapsed();

    assert!(
        matches!(&error, HerdrError::Timeout { method: "ping", .. }),
        "expected a timeout, got {error:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(200) && elapsed < Duration::from_secs(2),
        "the deadline must be the one that fired, not the harness's: {elapsed:?}"
    );
    assert_eq!(
        fake.received().len(),
        1,
        "the request did reach the server; only the answer never came"
    );
}

/// §9 row 2. A stale or missing socket is `Unavailable::NoSocket` **carrying the path**, reported
/// on the first call. Never `Ok`, never a retry loop, never a bare "herdr is not running".
#[tokio::test]
async fn a_dead_socket_is_unavailable_not_a_hang() {
    let fake = FakeHerdr::start_with(|request| pong_for(request, None));
    let dead = fake.dead_path();

    let error = ping(&dead).await.unwrap_err();

    let Some(Unavailable::NoSocket { path, .. }) = error.unavailable() else {
        panic!("expected NoSocket, got {error:?}");
    };
    assert_eq!(path, &dead);
    assert!(
        error.to_string().contains(&dead.display().to_string()),
        "the path is the actionable half of the sentence: {error}"
    );
    assert!(
        fake.received().is_empty(),
        "the live socket next door must not be touched"
    );
}

/// The pin: **not in a pane means no client, no connect, no task.**
///
/// The environment here is hostile on purpose — a live herdr socket is named in
/// `HERDR_SOCKET_PATH` and a fake server is listening on it — and `HERDR_ENV` is simply absent, as
/// it is in this container, in CI, and in any plain terminal. Three assertions, each of which a
/// different mistake would break:
///
/// 1. `discover` is `None` — the gate itself.
/// 2. the fake recorded **nothing** — no connection was opened. This is the assertion that fails if
///    discovery falls through to a default socket path, or if the asked-for path connects before
///    checking the gate.
/// 3. the runtime's live task count is unchanged across the whole sequence — no background task.
#[tokio::test]
async fn no_herdr_env_means_no_client_no_connect_and_no_task() {
    let fake = FakeHerdr::start_with(|request| pong_for(request, None));
    let socket = fake.path().display().to_string();
    let scrubbed = env(&[(HERDR_SOCKET_PATH, &socket), ("HOME", "/home/nobody")]);

    let metrics = tokio::runtime::Handle::current().metrics();
    let before = metrics.num_alive_tasks();

    assert!(
        crate::HerdrPane::discover(&scrubbed).is_none(),
        "HERDR_ENV is absent: this is not a herdr pane"
    );
    let error = ping_current_pane(&scrubbed).await.unwrap_err();
    assert!(matches!(
        error.unavailable(),
        Some(Unavailable::NotInHerdrPane)
    ));

    assert!(
        fake.received().is_empty(),
        "nothing may connect outside a herdr pane, and a live socket in HERDR_SOCKET_PATH is not \
         permission to: {:?}",
        fake.received()
    );
    assert_eq!(
        metrics.num_alive_tasks(),
        before,
        "the inert path spawns no task"
    );
}

/// The counterpart: inside a pane, the same code does connect, and `HERDR_PANE_ID` rides along.
#[tokio::test]
async fn inside_a_pane_the_probe_reaches_the_socket() {
    let fake = FakeHerdr::start_with(|request| pong_for(request, None));
    let socket = fake.path().display().to_string();
    let inside = env(&[
        (HERDR_ENV, "1"),
        (HERDR_PANE_ID, "w1:p1"),
        (HERDR_SOCKET_PATH, &socket),
    ]);

    let pong = ping_current_pane(&inside)
        .await
        .expect("inside a pane the probe reaches the fake");

    assert_eq!(pong.version(), "0.9.1");
    assert_eq!(fake.received().len(), 1);
}
