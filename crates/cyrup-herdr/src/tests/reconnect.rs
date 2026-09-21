//! [`ReconnectingEvents`]: a herdr restart refreshes the snapshot, and the retrying is bounded.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::fake_server::{
    FakeHerdr, Reply, agent_status_changed_event, empty_snapshot_result, error_for,
    pane_created_event, request_method, result_for, script, subscription_ack_for,
};
use crate::error::{ApiErrorCode, HerdrError};
use crate::reconnect::{
    Backoff, RECONNECT_ATTEMPTS, RECONNECT_INITIAL_DELAY, RECONNECT_MAX_DELAY, ReconnectingEvents,
    StreamItem,
};
use crate::schema::events::Subscription;
use crate::{HerdrClient, Unavailable};

/// A backoff small enough to run on a test clock but still a real schedule: 40 ms doubling to
/// 160 ms, giving up after three attempts.
const TEST_BACKOFF: Backoff = Backoff {
    initial: Duration::from_millis(40),
    max: Duration::from_millis(160),
    attempts: 3,
};

/// The schedule's floor, given [`Backoff::delay`]'s half-jitter: (40 + 80 + 160) / 2.
const TEST_BACKOFF_FLOOR: Duration = Duration::from_millis(140);

#[tokio::test]
async fn a_reconnect_refetches_the_snapshot() {
    // herdr says so — "Call `session.snapshot` again after reconnecting" (`socket-api.mdx:125`) —
    // and its source says why: a new subscription's floor is `current_sequence()` at accept time
    // (`tmp/herdr/src/api/server.rs:723`), so nothing that happened while the client was away is
    // replayed. Reconnecting the stream alone keeps a cache that can never be corrected.
    let subscribes = Arc::new(AtomicUsize::new(0));
    let snapshots = Arc::new(AtomicUsize::new(0));
    let (subscribe_count, snapshot_count) = (Arc::clone(&subscribes), Arc::clone(&snapshots));

    let fake = FakeHerdr::start_with(move |request| match request_method(request).as_str() {
        "events.subscribe" => {
            let nth = subscribe_count.fetch_add(1, Ordering::SeqCst);
            let ack = subscription_ack_for(request);
            script(move |mut handle| {
                let ack = ack.clone();
                async move {
                    handle.write_line(&ack).await;
                    if nth == 0 {
                        // The first stream carries one event and then herdr restarts under it.
                        handle.write_line(&pane_created_event("w1:p2")).await;
                    } else {
                        handle
                            .write_line(&agent_status_changed_event("w1:p1", "done"))
                            .await;
                        handle.hold().await;
                    }
                }
            })
        }
        "session.snapshot" => {
            snapshot_count.fetch_add(1, Ordering::SeqCst);
            result_for(request, &empty_snapshot_result())
        }
        other => panic!("unexpected method {other}"),
    });

    let client = HerdrClient::new(fake.path().to_path_buf());
    let (first, mut stream) = ReconnectingEvents::connect_with(
        client,
        vec![
            Subscription::PaneCreated {},
            Subscription::pane_agent_status_changed("w1:p1", None),
        ],
        TEST_BACKOFF,
    )
    .await
    .expect("the first bootstrap succeeds");
    assert_eq!(first.version, "0.9.1");
    assert_eq!(snapshots.load(Ordering::SeqCst), 1);

    let before_the_cut = stream.next().await.expect("an item").expect("no failure");
    assert!(
        matches!(&before_the_cut, StreamItem::Event(event) if event.name() == "pane.created"),
        "got {before_the_cut:?}"
    );

    let recovered = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("the reconnect completes inside the schedule")
        .expect("an item")
        .expect("the second bootstrap succeeds");
    assert!(
        matches!(recovered, StreamItem::Snapshot(_)),
        "a reconnect hands the consumer a FRESH snapshot before it hands it another event, \
         so a stale cache cannot be kept silently — got {recovered:?}"
    );
    assert_eq!(
        snapshots.load(Ordering::SeqCst),
        2,
        "the reconnect issued a second session.snapshot"
    );
    assert_eq!(subscribes.load(Ordering::SeqCst), 2);

    let after_the_cut = stream.next().await.expect("an item").expect("no failure");
    assert!(
        matches!(&after_the_cut, StreamItem::Event(event)
            if event.name() == "pane.agent_status_changed"),
        "got {after_the_cut:?}"
    );
}

#[tokio::test]
async fn reconnect_backoff_is_bounded_and_gives_up_loudly() {
    // A herdr the user has quit is not coming back. Retrying it forever is a process that never
    // stops and never says anything; the consumer gets a terminal `Err` instead, after a schedule
    // it can be held to on the clock.
    let subscribes = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&subscribes);
    let fake = FakeHerdr::start_with(move |request| match request_method(request).as_str() {
        "events.subscribe" => {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                let ack = subscription_ack_for(request);
                script(move |mut handle| {
                    let ack = ack.clone();
                    async move { handle.write_line(&ack).await }
                })
            } else {
                // herdr is there but refusing — and refusing with something that is NOT the way
                // the stream ended, so that "the LAST failure is the one reported" below is a
                // claim with a way to be wrong. The stream ends `Closed`; every retry answers an
                // `server_unavailable` envelope, which is `HerdrError::Api`.
                error_for(request, "server_unavailable", "herdr is restarting")
            }
        }
        "session.snapshot" => result_for(request, &empty_snapshot_result()),
        other => panic!("unexpected method {other}"),
    });

    let client = HerdrClient::new(fake.path().to_path_buf());
    let (_snapshot, mut stream) =
        ReconnectingEvents::connect_with(client, vec![Subscription::PaneCreated {}], TEST_BACKOFF)
            .await
            .expect("the first bootstrap succeeds");

    // The upper bound matters as much as the floor: an unbounded retry loop never reaches this
    // line at all, and "never answers" is the failure mode the attempt budget exists to prevent.
    let started = Instant::now();
    let error = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("a bounded schedule ends; an unbounded one would still be retrying")
        .expect("giving up is said out loud, not by going quiet")
        .expect_err("every attempt failed");
    let elapsed = started.elapsed();

    // The LAST failure is the one reported, and it is a failure that ACTUALLY HAPPENED. The
    // stream ended with a close; every retry failed differently. A `rebootstrap` that kept the
    // reason it started with — or that seeded itself with a fabricated one — would say `Closed`
    // here.
    let HerdrError::Api { method, source } = &error else {
        panic!("expected the retry's own error envelope, got {error:?}");
    };
    assert_eq!(*method, "events.subscribe");
    assert_eq!(source.code, ApiErrorCode::ServerUnavailable);
    assert_eq!(source.message, "herdr is restarting");
    assert!(
        elapsed >= TEST_BACKOFF_FLOOR,
        "three attempts must be spaced by the schedule; {elapsed:?} < {TEST_BACKOFF_FLOOR:?}"
    );
    assert_eq!(
        subscribes.load(Ordering::SeqCst),
        1 + TEST_BACKOFF.attempts as usize,
        "the first subscription plus exactly `attempts` retries — not an unbounded loop"
    );
    assert!(
        stream.next().await.is_none(),
        "and then the stream is over, rather than retrying behind the consumer's back"
    );
}

/// With **no** attempt budget, nothing is retried — so what the consumer is told must be the
/// reason the stream ENDED, not a failure that never happened.
///
/// `rebootstrap` used to seed its `last` with a hand-built
/// `HerdrError::Closed { method: "events.subscribe" }`. With `attempts: 0` the loop never runs and
/// that placeholder went straight to the consumer: a claim that herdr closed a connection, when no
/// connection had been attempted and the stream had in fact died on an undecodable line. Here the
/// stream ends with [`HerdrError::Malformed`] and that is what comes back.
#[tokio::test]
async fn a_zero_attempt_budget_reports_why_the_stream_ended_not_a_close_that_never_happened() {
    let fake = FakeHerdr::start_with(move |request| match request_method(request).as_str() {
        "events.subscribe" => {
            let ack = subscription_ack_for(request);
            script(move |mut handle| {
                let ack = ack.clone();
                async move {
                    handle.write_line(&ack).await;
                    // Not an event, not an envelope, not JSON: `HerdrEvents::next` answers
                    // `Malformed` and the stream is over.
                    handle.write_line("{ this is not json").await;
                    handle.hold().await;
                }
            })
        }
        "session.snapshot" => result_for(request, &empty_snapshot_result()),
        other => panic!("unexpected method {other}"),
    });

    let (_snapshot, mut stream) = ReconnectingEvents::connect_with(
        HerdrClient::new(fake.path().to_path_buf()),
        vec![Subscription::PaneCreated {}],
        Backoff {
            attempts: 0,
            ..TEST_BACKOFF
        },
    )
    .await
    .expect("the first bootstrap succeeds");

    let error = tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("a zero budget answers immediately")
        .expect("giving up is said out loud")
        .expect_err("there was no attempt to succeed");
    assert!(
        matches!(
            &error,
            HerdrError::Malformed {
                method: "events.subscribe",
                ..
            }
        ),
        "the reason the stream ended, got {error:?}"
    );

    assert!(
        stream.next().await.is_none(),
        "and the stream is terminal from then on"
    );
}

#[tokio::test]
async fn the_first_bootstrap_is_not_retried() {
    // Nothing has been established yet, so "herdr is not there" is the caller's ordinary answer
    // and must arrive at once — not after a schedule sized for a restart that is not happening.
    let fake = FakeHerdr::start(Reply::Close);
    let dead = fake.dead_path();
    let client = HerdrClient::new(dead.clone());

    let started = Instant::now();
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        ReconnectingEvents::connect_with(
            client,
            vec![Subscription::PaneCreated {}],
            // A schedule far longer than this test will wait, so a first bootstrap that went
            // through the retry loop could not possibly answer in time.
            Backoff {
                initial: Duration::from_secs(30),
                max: Duration::from_secs(30),
                attempts: 9,
            },
        ),
    )
    .await
    .expect("the first bootstrap must not be put behind the reconnect schedule")
    .expect_err("nothing is listening");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the first bootstrap is answered, not retried"
    );
    let Some(Unavailable::NoSocket { path, .. }) = error.unavailable() else {
        panic!("got {error:?}");
    };
    assert_eq!(path, &dead);

    // And the same through the entry point a production caller actually reaches, which carries
    // the real schedule — 6 attempts at up to 8 s would be a minute of silence if it retried.
    let started = Instant::now();
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        ReconnectingEvents::connect(
            HerdrClient::new(dead.clone()),
            vec![Subscription::PaneCreated {}],
        ),
    )
    .await
    .expect("the default schedule is not applied to the first bootstrap either")
    .expect_err("nothing is listening");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(matches!(
        error.unavailable(),
        Some(Unavailable::NoSocket { .. })
    ));
}

#[test]
fn the_reconnect_schedule_is_a_quarter_second_doubling_to_eight_capped_and_jittered() {
    let backoff = Backoff::default();
    assert_eq!(backoff.initial, RECONNECT_INITIAL_DELAY);
    assert_eq!(backoff.max, RECONNECT_MAX_DELAY);
    assert_eq!(backoff.attempts, RECONNECT_ATTEMPTS);
    assert_eq!(RECONNECT_INITIAL_DELAY, Duration::from_millis(250));
    assert_eq!(RECONNECT_MAX_DELAY, Duration::from_secs(8));
    assert_eq!(RECONNECT_ATTEMPTS, 6);

    // The nominal schedule, doubling and then pinned at the ceiling.
    let nominal = [250_u64, 500, 1_000, 2_000, 4_000, 8_000, 8_000, 8_000];
    for (attempt, millis) in nominal.into_iter().enumerate() {
        let attempt = u32::try_from(attempt).expect("a small index");
        let nominal = Duration::from_millis(millis);
        // Sampled, because the jitter is drawn per call: every draw must land in the half-open
        // band, and the band must not collapse to a point (which is what "jittered" buys).
        let mut low = Duration::MAX;
        let mut high = Duration::ZERO;
        for _ in 0..256 {
            let delay = backoff.delay(attempt);
            assert!(
                delay >= nominal / 2 && delay <= nominal,
                "attempt {attempt}: {delay:?} outside [{:?}, {nominal:?}]",
                nominal / 2
            );
            low = low.min(delay);
            high = high.max(delay);
        }
        assert!(
            high > low,
            "attempt {attempt} produced a constant delay, which re-converges the herd it exists \
             to spread"
        );
    }

    // A far-future attempt cannot overflow its way past the ceiling.
    assert!(backoff.delay(u32::MAX) <= RECONNECT_MAX_DELAY);
}
