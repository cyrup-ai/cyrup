//! The no-gap bootstrap: the ordering herdr requires, driven through
//! [`crate::HerdrClient::bootstrap`] against a real socket.
//!
//! herdr's rule, verbatim: *"To avoid a bootstrap gap, first open `events.subscribe` on another
//! connection and wait for its acknowledgement. Buffer that stream while calling
//! `session.snapshot`, install the snapshot, then apply the buffered events in order and continue
//! streaming."* (`tmp/herdr/docs/preview/website/src/content/docs/socket-api.mdx:118-130`).
//!
//! The reason it is mandatory rather than defensive is one line of herdr's source:
//! `stream_subscriptions` takes `event_hub.current_sequence()` as its floor **before** it builds
//! any subscription (`tmp/herdr/src/api/server.rs:723`), so an event that fires before the
//! subscribe is accepted is never sent at all. There is no later correction and no replay.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::fake_server::{
    FakeHerdr, Reply, agent_status_changed_event, empty_snapshot_result, error_for,
    pane_created_event, request_method, result_for, script, subscription_ack_for,
};
use crate::error::{ApiErrorCode, HerdrError};
use crate::schema::events::{Event, Subscription};
use crate::{HerdrClient, Unavailable};

/// The two subscriptions every test here asks for — one lifecycle, one rich.
fn subscriptions() -> Vec<Subscription> {
    vec![
        Subscription::PaneCreated {},
        Subscription::pane_agent_status_changed("w1:p1", None),
    ]
}

#[tokio::test]
async fn bootstrap_opens_the_subscription_and_acks_it_before_the_snapshot() {
    // The journal records, in one order, the arrival of each request and the moment the
    // acknowledgement is written. A correct bootstrap can only produce one sequence.
    //
    // The 120 ms hold on the acknowledgement is what makes the middle entry load-bearing: a client
    // that opened both connections at once would have `session.snapshot` recorded before `ack`
    // even though `events.subscribe` arrived first.
    let journal: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&journal);
    let fake = FakeHerdr::start_with(move |request| {
        let recorder = Arc::clone(&recorder);
        match request_method(request).as_str() {
            "events.subscribe" => {
                recorder.lock().expect("journal").push("events.subscribe");
                let ack = subscription_ack_for(request);
                script(move |mut handle| {
                    let ack = ack.clone();
                    let recorder = Arc::clone(&recorder);
                    async move {
                        tokio::time::sleep(Duration::from_millis(120)).await;
                        handle.write_line(&ack).await;
                        recorder.lock().expect("journal").push("ack");
                        handle.hold().await;
                    }
                })
            }
            "session.snapshot" => {
                recorder.lock().expect("journal").push("session.snapshot");
                result_for(request, &empty_snapshot_result())
            }
            other => panic!("unexpected method {other}"),
        }
    });

    let client = HerdrClient::new(fake.path().to_path_buf());
    let (snapshot, events) = client
        .bootstrap(subscriptions())
        .await
        .expect("the fake answers both");

    assert_eq!(snapshot.version, "0.9.1");
    assert_eq!(snapshot.focused_pane_id.as_deref(), Some("w1:p1"));
    assert_eq!(
        journal.lock().expect("journal").as_slice(),
        ["events.subscribe", "ack", "session.snapshot"],
        "the subscription is opened AND acknowledged before the snapshot is asked for"
    );
    events.close();
}

#[tokio::test]
async fn bootstrap_delivers_an_event_that_arrived_between_the_ack_and_the_snapshot() {
    // The losing interleaving, made to happen on purpose: two events are pushed on the
    // subscription connection while the snapshot reply is being held. They must reach the caller
    // after the snapshot, in arrival order, with none dropped.
    let fake = FakeHerdr::start_with(|request| match request_method(request).as_str() {
        "events.subscribe" => {
            let ack = subscription_ack_for(request);
            script(move |mut handle| {
                let ack = ack.clone();
                async move {
                    handle.write_line(&ack).await;
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    handle.write_line(&pane_created_event("w1:p7")).await;
                    handle
                        .write_line(&agent_status_changed_event("w1:p1", "working"))
                        .await;
                    handle.hold().await;
                }
            })
        }
        "session.snapshot" => super::fake_server::result_after(
            request,
            Duration::from_millis(200),
            &empty_snapshot_result(),
        ),
        other => panic!("unexpected method {other}"),
    });

    let client = HerdrClient::new(fake.path().to_path_buf());
    let (snapshot, mut events) = client
        .bootstrap(subscriptions())
        .await
        .expect("the fake answers both");
    assert_eq!(snapshot.protocol, 22);

    // Both were on the wire before `bootstrap` returned, so they are available without waiting.
    let first = tokio::time::timeout(Duration::from_millis(50), events.next())
        .await
        .expect("the first event was already buffered")
        .expect("an event")
        .expect("decodes");
    let second = tokio::time::timeout(Duration::from_millis(50), events.next())
        .await
        .expect("the second event was already buffered")
        .expect("an event")
        .expect("decodes");
    assert_eq!(first.name(), "pane.created", "arrival order is preserved");
    assert_eq!(second.name(), "pane.agent_status_changed");
    assert!(matches!(first, Event::Lifecycle(_)));
    assert!(matches!(second, Event::Subscription(_)));
}

#[tokio::test]
async fn bootstrap_drains_the_stream_while_the_snapshot_is_held() {
    // Draining is not a nicety. herdr writes subscription events with a **blocking** write on a
    // socket whose send timeout it set to 5 s (`tmp/herdr/src/api/server.rs:164-166`, `:31`); a
    // client that waits for its snapshot before reading a single event therefore wedges herdr's
    // writer, and against a real herdr the subscription is dropped when that write times out.
    //
    // The fake reproduces exactly that dependency: it pushes more than one socket buffer's worth
    // of events and only answers `session.snapshot` once every one of them has been written. A
    // client that awaits the snapshot first and reads afterwards deadlocks and never gets a
    // snapshot at all.
    const EVENTS: usize = 64;
    let drained = Arc::new(tokio::sync::Notify::new());
    let signal = Arc::clone(&drained);
    let fake = FakeHerdr::start_with(move |request| {
        let signal = Arc::clone(&signal);
        match request_method(request).as_str() {
            "events.subscribe" => {
                let ack = subscription_ack_for(request);
                script(move |mut handle| {
                    let ack = ack.clone();
                    let signal = Arc::clone(&signal);
                    async move {
                        handle.write_line(&ack).await;
                        for index in 0..EVENTS {
                            // ~16 KiB each: 1 MiB in total, comfortably past an AF_UNIX buffer.
                            handle
                                .write_line(&super::fake_server::compact(&format!(
                                    r#"{{"event":"tab_renamed","data":{{"type":"tab_renamed",
                                       "tab_id":"w1:t{index}","workspace_id":"w1","label":"{}"}}}}"#,
                                    "p".repeat(16 * 1024)
                                )))
                                .await;
                        }
                        signal.notify_waiters();
                        handle.hold().await;
                    }
                })
            }
            "session.snapshot" => {
                let signal = Arc::clone(&signal);
                let answer = super::fake_server::compact(&format!(
                    r#"{{"id":"{}","result":{}}}"#,
                    super::fake_server::request_id(request),
                    empty_snapshot_result()
                ));
                script(move |mut handle| {
                    let answer = answer.clone();
                    let signal = Arc::clone(&signal);
                    async move {
                        signal.notified().await;
                        handle.write_line(&answer).await;
                    }
                })
            }
            other => panic!("unexpected method {other}"),
        }
    });
    drop(drained);

    let client = HerdrClient::new(fake.path().to_path_buf());
    let (snapshot, mut events) = tokio::time::timeout(
        Duration::from_secs(5),
        client.bootstrap(vec![Subscription::TabRenamed {}]),
    )
    .await
    .expect("the snapshot cannot arrive unless the stream is being read at the same time")
    .expect("the fake answers both");
    assert_eq!(snapshot.version, "0.9.1");

    let mut seen = 0_usize;
    while seen < EVENTS {
        let event = tokio::time::timeout(Duration::from_secs(5), events.next())
            .await
            .expect("every pushed event is still there")
            .expect("an event")
            .expect("decodes");
        assert_eq!(event.name(), "tab.renamed");
        seen += 1;
    }
    assert_eq!(seen, EVENTS, "none was dropped on the way");
}

#[tokio::test]
async fn a_refused_snapshot_fails_the_whole_bootstrap() {
    // A bootstrap is a pair or it is nothing: half of it is a stream whose cache was never built.
    let fake = FakeHerdr::start_with(|request| match request_method(request).as_str() {
        "events.subscribe" => {
            let ack = subscription_ack_for(request);
            script(move |mut handle| {
                let ack = ack.clone();
                async move {
                    handle.write_line(&ack).await;
                    handle.hold().await;
                }
            })
        }
        "session.snapshot" => error_for(request, "ui_busy", "the ui is busy"),
        other => panic!("unexpected method {other}"),
    });

    let client = HerdrClient::new(fake.path().to_path_buf());
    let error = client
        .bootstrap(subscriptions())
        .await
        .expect_err("a refused snapshot is not half a bootstrap");
    assert!(
        matches!(
            &error,
            HerdrError::Api { method: "session.snapshot", source }
                if source.code == ApiErrorCode::UiBusy && source.message == "the ui is busy"
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_dead_socket_names_the_path_on_a_bootstrap() {
    let fake = FakeHerdr::start(Reply::Close);
    let dead = fake.dead_path();
    let client = HerdrClient::new(dead.clone());

    let error = client
        .bootstrap(subscriptions())
        .await
        .expect_err("nothing is listening");
    let Some(Unavailable::NoSocket { path, .. }) = error.unavailable() else {
        panic!("got {error:?}");
    };
    assert_eq!(path, &dead, "the path is the actionable half");
    assert!(
        fake.received().is_empty(),
        "and the live socket beside it was never touched"
    );
}
