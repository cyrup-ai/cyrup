//! `events.subscribe`: the handshake, the two spelling conventions, and every way the stream ends.
//!
//! Every test drives a real `UnixListener` through [`crate::HerdrClient::subscribe`] — the public
//! entry point — so there is no mock between the assertion and the wire. The fake speaks
//! `stream_subscriptions`' real shape (`tmp/herdr/src/api/server.rs:715-779`): one ack line, then
//! pushed event lines on the same connection, with **no `id` field** on any of them.

use std::time::Duration;

use super::fake_server::{
    FakeHerdr, Reply, agent_status_changed_event, compact, error_for, pane_created_event,
    request_method, request_params, script, subscription_ack_for,
};
use crate::error::{ApiErrorCode, HerdrError};
use crate::schema::common::{AgentStatus, ReadSource};
use crate::schema::events::{
    Event, EventData, EventKind, OutputMatch, Subscription, SubscriptionEventData,
    SubscriptionEventKind,
};
use crate::transport::MAX_RESPONSE_BYTES;
use crate::{HerdrClient, MAX_STREAM_BYTES};

/// A fake that acknowledges the subscription and then pushes `lines`, in order, and holds.
fn fake_pushing(lines: Vec<String>) -> FakeHerdr {
    FakeHerdr::start_with(move |request| {
        let ack = subscription_ack_for(request);
        let lines = lines.clone();
        script(move |mut handle| {
            let ack = ack.clone();
            let lines = lines.clone();
            async move {
                handle.write_line(&ack).await;
                for line in lines {
                    handle.write_line(&line).await;
                }
                handle.hold().await;
            }
        })
    })
}

#[tokio::test]
async fn subscribe_sends_herdrs_own_subscription_vocabulary_with_dots() {
    let fake = fake_pushing(Vec::new());
    let client = HerdrClient::new(fake.path().to_path_buf());

    let events = client
        .subscribe(vec![
            Subscription::PaneCreated {},
            Subscription::pane_agent_status_changed("w1:p1", Some(AgentStatus::Blocked)),
            Subscription::pane_output_matched(
                "w1:p2",
                ReadSource::RecentUnwrapped,
                OutputMatch::substring("ready"),
            ),
            Subscription::LayoutUpdated {},
        ])
        .await
        .expect("the fake acknowledges");

    let received = fake.received();
    let [request] = received.as_slice() else {
        panic!("exactly one request, got {received:?}");
    };
    assert_eq!(request_method(request), "events.subscribe");
    // The request half is the half a client cannot see from its own return value, so it is
    // asserted whole: every `type` dotted, `agent_status` snake_case, `match` spelled `match`.
    assert_eq!(
        request_params(request),
        serde_json::from_str::<serde_json::Value>(&compact(
            r#"{"subscriptions":[
                 {"type":"pane.created"},
                 {"type":"pane.agent_status_changed","pane_id":"w1:p1","agent_status":"blocked"},
                 {"type":"pane.output_matched","pane_id":"w1:p2","source":"recent_unwrapped",
                  "match":{"type":"substring","value":"ready"},"strip_ansi":true},
                 {"type":"layout.updated"}]}"#
        ))
        .expect("the expectation is valid JSON")
    );
    events.close();
}

#[tokio::test]
async fn every_subscription_this_client_offers_is_one_herdr_accepts() {
    // herdr's own `Subscription` renames, `tmp/herdr/src/api/schema/events.rs:16-83`, in
    // declaration order. A typo in a rename is invisible until a real herdr answers
    // `invalid_request` for a subscription that looks fine in Rust, so the whole table is pinned.
    let expected = [
        (Subscription::WorkspaceCreated {}, "workspace.created"),
        (Subscription::WorkspaceUpdated {}, "workspace.updated"),
        (
            Subscription::WorkspaceMetadataUpdated {},
            "workspace.metadata_updated",
        ),
        (Subscription::WorkspaceRenamed {}, "workspace.renamed"),
        (Subscription::WorkspaceMoved {}, "workspace.moved"),
        (Subscription::WorkspaceReordered {}, "workspace.reordered"),
        (Subscription::WorkspaceClosed {}, "workspace.closed"),
        (Subscription::WorkspaceFocused {}, "workspace.focused"),
        (Subscription::WorktreeCreated {}, "worktree.created"),
        (Subscription::WorktreeOpened {}, "worktree.opened"),
        (Subscription::WorktreeRemoved {}, "worktree.removed"),
        (Subscription::TabCreated {}, "tab.created"),
        (Subscription::TabClosed {}, "tab.closed"),
        (Subscription::TabFocused {}, "tab.focused"),
        (Subscription::TabRenamed {}, "tab.renamed"),
        (Subscription::TabMoved {}, "tab.moved"),
        (Subscription::PaneCreated {}, "pane.created"),
        (Subscription::PaneClosed {}, "pane.closed"),
        (Subscription::PaneUpdated {}, "pane.updated"),
        (Subscription::PaneFocused {}, "pane.focused"),
        (Subscription::PaneMoved {}, "pane.moved"),
        (Subscription::PaneExited {}, "pane.exited"),
        (Subscription::PaneAgentDetected {}, "pane.agent_detected"),
        (
            Subscription::pane_output_matched(
                "w1:p1",
                ReadSource::Visible,
                OutputMatch::regex("^ok$"),
            ),
            "pane.output_matched",
        ),
        (
            Subscription::pane_agent_status_changed("w1:p1", None),
            "pane.agent_status_changed",
        ),
        (
            Subscription::pane_scroll_changed("w1:p1"),
            "pane.scroll_changed",
        ),
        (Subscription::LayoutUpdated {}, "layout.updated"),
    ];
    assert_eq!(expected.len(), 27, "herdr declares 27 subscriptions");
    for (subscription, name) in expected {
        let value = serde_json::to_value(&subscription).expect("a subscription serialises");
        assert_eq!(
            value.get("type").and_then(serde_json::Value::as_str),
            Some(name),
            "{subscription:?} must ask for {name}"
        );
    }
}

#[tokio::test]
async fn the_subscription_ack_must_be_subscription_started() {
    // pi pins the same thing (`src/runs/shared/herdr-connection.ts:100` @v0.68.0). A success that
    // is not the ack means this connection is in a state the client cannot name, and handing back
    // a live `HerdrEvents` for it would be a subscription herdr never started.
    let fake = FakeHerdr::start_with(|request| {
        Reply::Line(compact(&format!(
            r#"{{"id":"{}","result":{{"type":"pong","version":"0.9.1","protocol":22}}}}"#,
            super::fake_server::request_id(request)
        )))
    });
    let client = HerdrClient::new(fake.path().to_path_buf());

    let error = client
        .subscribe(vec![Subscription::PaneCreated {}])
        .await
        .expect_err("a pong is not an acknowledgement");
    assert!(
        matches!(
            &error,
            HerdrError::UnexpectedResult {
                method: "events.subscribe",
                want: "subscription_started",
                got,
            } if got == "pong"
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn an_uncorrelated_invalid_request_fails_the_handshake() {
    // The shape herdr writes when the line did not deserialise and it could not recover an id at
    // all (`tmp/herdr/src/api/server.rs:180-201`). Requiring a matching id before treating an
    // error as fatal makes this shape invisible and the handshake waits out its deadline instead.
    let fake = FakeHerdr::start(Reply::Line(compact(
        r#"{"id":"","error":{"code":"invalid_request","message":"invalid request: unknown variant"}}"#,
    )));
    let client = HerdrClient::new(fake.path().to_path_buf());

    let started = std::time::Instant::now();
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        client.subscribe(vec![Subscription::PaneCreated {}]),
    )
    .await
    .expect("the handshake must fail at once, not wait out its deadline")
    .expect_err("an error envelope is not a stream");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(
        matches!(
            &error,
            HerdrError::Api { method: "events.subscribe", source }
                if source.code == ApiErrorCode::InvalidRequest
                    && source.message == "invalid request: unknown variant"
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_mismatched_ack_id_is_refused() {
    let fake = FakeHerdr::start(Reply::Line(compact(
        r#"{"id":"someone_else","result":{"type":"subscription_started"}}"#,
    )));
    let client = HerdrClient::new(fake.path().to_path_buf());

    let error = client
        .subscribe(vec![Subscription::PaneCreated {}])
        .await
        .expect_err("another request's acknowledgement is not this one's");
    assert!(
        matches!(
            &error,
            HerdrError::IdMismatch { method: "events.subscribe", got, .. } if got == "someone_else"
        ),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_lifecycle_event_uses_underscores_and_a_subscription_event_uses_dots() {
    // The naming trap: both envelopes ride one connection and spell their names differently.
    // Giving `EventKind` dotted renames — the plausible mistake, since the *request* uses dots —
    // makes the first line undecodable.
    let fake = fake_pushing(vec![
        pane_created_event("w1:p3"),
        agent_status_changed_event("w1:p1", "blocked"),
    ]);
    let client = HerdrClient::new(fake.path().to_path_buf());
    let mut events = client
        .subscribe(vec![
            Subscription::PaneCreated {},
            Subscription::pane_agent_status_changed("w1:p1", None),
        ])
        .await
        .expect("acknowledged");

    let first = events
        .next()
        .await
        .expect("a first event")
        .expect("decodes");
    let Event::Lifecycle(envelope) = first else {
        panic!("pane_created is a lifecycle event, got {first:?}");
    };
    assert_eq!(envelope.event, EventKind::PaneCreated);
    assert_eq!(envelope.event.dot_name(), "pane.created");
    let EventData::PaneCreated { pane } = &envelope.data else {
        panic!("got {:?}", envelope.data);
    };
    assert_eq!(pane.pane_id, "w1:p3");
    assert_eq!(pane.revision, 7);

    let second = events
        .next()
        .await
        .expect("a second event")
        .expect("decodes");
    let Event::Subscription(envelope) = second else {
        panic!("pane.agent_status_changed is a subscription event, got {second:?}");
    };
    assert_eq!(
        envelope.event,
        SubscriptionEventKind::PaneAgentStatusChanged
    );
    let SubscriptionEventData::PaneAgentStatusChanged(changed) = &envelope.data else {
        panic!("got {:?}", envelope.data);
    };
    assert_eq!(changed.pane_id, "w1:p1");
    assert_eq!(changed.workspace_id, "w1");
    assert_eq!(changed.agent_status, AgentStatus::Blocked);
}

#[tokio::test]
async fn an_event_name_this_client_does_not_know_is_unrecognised_not_a_dead_stream() {
    // A 27th event kind from a newer herdr must not sever a subscription that is otherwise
    // working: the unknown line is handed over verbatim and the next known one still decodes.
    let fake = fake_pushing(vec![
        compact(
            r#"{"event":"pane_teleported","data":{"type":"pane_teleported","pane_id":"w1:p9"}}"#,
        ),
        pane_created_event("w1:p4"),
    ]);
    let client = HerdrClient::new(fake.path().to_path_buf());
    let mut events = client
        .subscribe(vec![Subscription::PaneCreated {}])
        .await
        .expect("acknowledged");

    let first = events.next().await.expect("a first line").expect("decodes");
    let Event::Unrecognised(value) = &first else {
        panic!("an unknown kind is Unrecognised, got {first:?}");
    };
    assert_eq!(first.name(), "pane_teleported");
    assert_eq!(
        value.get("data").and_then(|data| data.get("pane_id")),
        Some(&serde_json::Value::String("w1:p9".to_owned())),
        "the line is kept verbatim, not summarised"
    );

    let second = events
        .next()
        .await
        .expect("the stream is alive")
        .expect("decodes");
    assert!(matches!(second, Event::Lifecycle(_)), "got {second:?}");
}

#[tokio::test]
async fn a_known_event_whose_payload_changed_is_malformed_not_unrecognised() {
    // The other direction of the same rule: an `event` name this client knows, carrying a payload
    // it cannot read, is a herdr that changed a payload under a name it kept. Folding that into
    // `Unrecognised` would report a changed event as a readable one.
    let fake = fake_pushing(vec![compact(
        r#"{"event":"pane_created","data":{"type":"pane_created"}}"#,
    )]);
    let client = HerdrClient::new(fake.path().to_path_buf());
    let mut events = client
        .subscribe(vec![Subscription::PaneCreated {}])
        .await
        .expect("acknowledged");

    let error = events
        .next()
        .await
        .expect("a line")
        .expect_err("a known kind with an unreadable payload is not an event");
    let HerdrError::Malformed { method, source } = &error else {
        panic!("got {error:?}");
    };
    assert_eq!(*method, "events.subscribe");
    assert!(
        source.to_string().contains("pane"),
        "serde's own message must name the missing field, got {source}"
    );
    assert!(
        events.next().await.is_none(),
        "every error on this stream is terminal"
    );
}

#[tokio::test]
async fn a_closed_stream_yields_closed_then_none() {
    let fake = FakeHerdr::start_with(|request| {
        let ack = subscription_ack_for(request);
        script(move |mut handle| {
            let ack = ack.clone();
            async move {
                handle.write_line(&ack).await;
                // and close, which is what a herdr restart looks like from here
            }
        })
    });
    let client = HerdrClient::new(fake.path().to_path_buf());
    let mut events = client
        .subscribe(vec![Subscription::PaneCreated {}])
        .await
        .expect("acknowledged");

    let error = events
        .next()
        .await
        .expect("an end is reported before it is silent")
        .expect_err("a closed connection is not an event");
    assert!(
        matches!(
            error,
            HerdrError::Closed {
                method: "events.subscribe"
            }
        ),
        "got {error:?}"
    );
    assert!(events.next().await.is_none(), "and then it is over");
}

#[tokio::test]
async fn an_error_envelope_after_the_ack_ends_the_stream() {
    let fake = fake_pushing(vec![compact(
        r#"{"id":"","error":{"code":"server_unavailable","message":"herdr is shutting down"}}"#,
    )]);
    let client = HerdrClient::new(fake.path().to_path_buf());
    let mut events = client
        .subscribe(vec![Subscription::PaneCreated {}])
        .await
        .expect("acknowledged");

    let error = events
        .next()
        .await
        .expect("a line")
        .expect_err("an error envelope is not an event");
    assert!(
        matches!(
            &error,
            HerdrError::Api { method: "events.subscribe", source }
                if source.code == ApiErrorCode::ServerUnavailable
                    && source.message == "herdr is shutting down"
        ),
        "got {error:?}"
    );
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn a_stream_line_over_four_mib_is_refused() {
    let oversized = format!(
        r#"{{"event":"tab_renamed","data":{{"type":"tab_renamed","tab_id":"w1:t1","workspace_id":"w1","label":"{}"}}}}"#,
        "x".repeat(MAX_RESPONSE_BYTES + 16)
    );
    let fake = fake_pushing(vec![oversized]);
    let client = HerdrClient::new(fake.path().to_path_buf());
    let mut events = client
        .subscribe(vec![Subscription::TabRenamed {}])
        .await
        .expect("acknowledged");

    let error = events
        .next()
        .await
        .expect("a line")
        .expect_err("an unbounded read on a socket is an unbounded allocation");
    assert!(
        matches!(
            error,
            HerdrError::TooLarge {
                method: "events.subscribe",
                limit: MAX_RESPONSE_BYTES,
            }
        ),
        "got {error:?}"
    );
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn the_stream_recycles_itself_once_it_has_carried_its_byte_budget() {
    // `MAX_STREAM_BYTES` is a recycle threshold, not a memory bound — see `crate::stream`. It is
    // driven for real here rather than asserted as a constant: a threshold that is never crossed
    // in a test is a threshold nobody has proven is reachable.
    const CHUNK: usize = 4 * 1024 * 1024 - 4096;
    let line = format!(
        r#"{{"event":"nothing_this_client_knows","padding":"{}"}}"#,
        "x".repeat(CHUNK)
    );
    let needed = usize::try_from(MAX_STREAM_BYTES).unwrap_or(usize::MAX) / line.len() + 2;
    let fake = FakeHerdr::start_with(move |request| {
        let ack = subscription_ack_for(request);
        let line = line.clone();
        script(move |mut handle| {
            let ack = ack.clone();
            let line = line.clone();
            async move {
                handle.write_line(&ack).await;
                for _ in 0..needed {
                    handle.write_line(&line).await;
                }
                handle.hold().await;
            }
        })
    });
    let client = HerdrClient::new(fake.path().to_path_buf());
    let mut events = client
        .subscribe(vec![Subscription::PaneUpdated {}])
        .await
        .expect("acknowledged");

    let mut seen = 0_u32;
    let error = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match events.next().await.expect("the stream ends loudly") {
                Ok(_) => seen += 1,
                Err(error) => break error,
            }
        }
    })
    .await
    .expect("the budget is a cut, not a stream that runs until the harness gives up");
    assert!(seen > 0, "the padding events were delivered before the cut");
    assert!(
        events.bytes_read() > MAX_STREAM_BYTES,
        "the cut is taken on the byte counter, at {} bytes",
        events.bytes_read()
    );
    assert!(
        matches!(&error, HerdrError::TooLarge { method: "events.subscribe", limit }
            if *limit == usize::try_from(MAX_STREAM_BYTES).unwrap_or(usize::MAX)),
        "got {error:?}"
    );
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn a_refused_subscription_carries_herdrs_own_code_and_message() {
    // herdr refuses a subscription it cannot build — `ActiveSubscription::new` probes the pane and
    // answers the failure with the request's own id (`tmp/herdr/src/api/server.rs:725-748`).
    let fake = FakeHerdr::start_with(|request| {
        error_for(request, "pane_not_found", "pane w1:p9 not found")
    });
    let client = HerdrClient::new(fake.path().to_path_buf());

    let error = client
        .subscribe(vec![Subscription::pane_scroll_changed("w1:p9")])
        .await
        .expect_err("a refused subscription is not a stream");
    let HerdrError::Api { method, source } = &error else {
        panic!("got {error:?}");
    };
    assert_eq!(*method, "events.subscribe");
    assert_eq!(source.code, ApiErrorCode::PaneNotFound);
    assert_eq!(source.message, "pane w1:p9 not found");
}
