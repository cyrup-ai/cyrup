//! [`ReconnectingEvents`] — a herdr restart is a new snapshot, not a resumed stream.
//!
//! ## Why a reconnect must re-snapshot
//!
//! herdr says so in its own prose — *"Call `session.snapshot` again after reconnecting or when the
//! local cache may be stale."*
//! (`tmp/herdr/docs/preview/website/src/content/docs/socket-api.mdx:125-126`) — and its source
//! says why: a new subscription's floor is `event_hub.current_sequence()` taken when the request
//! is accepted (`tmp/herdr/src/api/server.rs:723`), so nothing that happened while the client was
//! away is replayed. A consumer that reconnected the stream alone would hold a cache describing a
//! session that has since moved, with no event ever arriving to correct it — a stale cache kept
//! silently, which is the one outcome this crate refuses.
//!
//! So a reconnect here is a full [`crate::HerdrClient::bootstrap`], and the fresh snapshot is
//! handed to the consumer **in band** as [`StreamItem::Snapshot`]. There is no way to keep reading
//! events across a reconnect without being told to replace the cache.
//!
//! ## Bounded, and loud when it gives up
//!
//! The backoff is capped and the attempts are counted. An unbounded retry loop against a herdr the
//! user has quit is a process that never stops and never says anything; when [`Backoff::attempts`]
//! consecutive bootstraps have failed, the last failure is handed to the consumer as a terminal
//! `Err` and the stream ends. A consumer that wants to keep trying beyond that is deciding to, in
//! code that can be read.
//!
//! The jitter is not decoration: every cyrup process in every pane of a herdr that just restarted
//! wakes at the same instant, and an unjittered schedule would have all of them re-bootstrap in
//! lockstep — each re-bootstrap being two connections and a `session.snapshot`, the most expensive
//! call on the socket.

use std::time::Duration;

use crate::client::HerdrClient;
use crate::error::{HerdrError, Result};
use crate::schema::events::{Event, Subscription};
use crate::schema::session::SessionSnapshot;
use crate::stream::HerdrEvents;

/// The first delay after a stream ends — 250 ms.
pub const RECONNECT_INITIAL_DELAY: Duration = Duration::from_millis(250);

/// The ceiling each doubling is capped at — 8 s.
pub const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(8);

/// How many consecutive failed bootstraps end the stream — 6.
///
/// With the default schedule that is 250 ms + 500 ms + 1 s + 2 s + 4 s + 8 s ≈ 15.75 s of trying
/// before giving up, halved at worst by the jitter. It is sized for *a herdr that is restarting*,
/// which is the case this exists for; a herdr the user has **quit** is not coming back, and
/// waiting longer for it only delays telling the consumer so.
pub const RECONNECT_ATTEMPTS: u32 = 6;

/// A capped, jittered exponential backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// The first delay.
    pub initial: Duration,
    /// The ceiling every delay is capped at.
    pub max: Duration,
    /// How many consecutive failures end the stream.
    pub attempts: u32,
}

impl Default for Backoff {
    /// herdr's restart, sized: [`RECONNECT_INITIAL_DELAY`] → [`RECONNECT_MAX_DELAY`], giving up
    /// after [`RECONNECT_ATTEMPTS`].
    fn default() -> Self {
        Self {
            initial: RECONNECT_INITIAL_DELAY,
            max: RECONNECT_MAX_DELAY,
            attempts: RECONNECT_ATTEMPTS,
        }
    }
}

impl Backoff {
    /// The delay before attempt `attempt` (0-based): `initial * 2^attempt`, capped at `max`, then
    /// jittered down into `[delay / 2, delay]`.
    ///
    /// Half-jitter rather than full: a delay that can be near zero re-converges the herd it is
    /// there to spread, and a delay that is never less than half the schedule keeps the "bounded"
    /// half of the promise checkable — [`Self::attempts`] delays are always at least half the
    /// nominal schedule, which is what
    /// `reconnect_backoff_is_bounded_and_gives_up_loudly` asserts on the clock.
    #[must_use]
    pub fn delay(self, attempt: u32) -> Duration {
        let scaled = self
            .initial
            .checked_mul(1_u32.checked_shl(attempt.min(31)).unwrap_or(u32::MAX))
            .unwrap_or(self.max)
            .min(self.max);
        let nanos = u64::from(scaled.subsec_nanos()).saturating_add(
            scaled
                .as_secs()
                .saturating_mul(u64::from(NANOS_PER_SEC))
                .min(u64::MAX / 2),
        );
        let half = nanos / 2;
        Duration::from_nanos(half.saturating_add(jitter_below(half.saturating_add(1))))
    }
}

const NANOS_PER_SEC: u32 = 1_000_000_000;

/// A number in `0..bound`, spread well enough to decorrelate a herd of panes waking together.
///
/// Not a general-purpose RNG and not seeded from one: this crate is a leaf with five dependencies
/// (see its `Cargo.toml`) and a reconnect delay is not a place that needs a sixth. The wall clock's
/// nanosecond field plus a per-process counter is uncorrelated between processes, which is the
/// only property being asked for.
fn jitter_below(bound: u64) -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);

    let clock = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::from(since.subsec_nanos()));
    let mixed = COUNTER
        .fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed)
        .rotate_left(17)
        ^ clock.wrapping_mul(0x2545_F491_4F6C_DD1D);
    if bound == 0 { 0 } else { mixed % bound }
}

/// What a [`ReconnectingEvents`] hands its consumer.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamItem {
    /// A fresh `session.snapshot`: **replace the cache with this**, then keep applying events.
    ///
    /// Produced once per successful (re)bootstrap after the first. It is not an event and it is
    /// not optional — receiving it is the only correct moment to discard what the previous stream
    /// described.
    Snapshot(Box<SessionSnapshot>),
    /// One event from the live stream.
    Event(Event),
}

/// A `bootstrap` that re-runs itself when herdr restarts.
///
/// Built by [`Self::connect`], which performs the first bootstrap and hands back its snapshot
/// separately — so the first snapshot is a value the caller cannot ignore, and every later one
/// arrives as [`StreamItem::Snapshot`] in the same stream as the events it precedes.
#[derive(Debug)]
pub struct ReconnectingEvents {
    client: HerdrClient,
    subscriptions: Vec<Subscription>,
    backoff: Backoff,
    events: Option<HerdrEvents>,
    /// Set once the attempt budget is spent; `next` answers `None` from then on.
    finished: bool,
}

impl ReconnectingEvents {
    /// Bootstrap, and keep the pieces needed to do it again.
    ///
    /// The first snapshot is returned rather than streamed: a consumer has nothing to replace yet,
    /// and a `StreamItem::Snapshot` it had to wait for would let it start applying events to an
    /// empty cache.
    ///
    /// # Errors
    /// As [`HerdrClient::bootstrap`] — the **first** bootstrap is not retried. Nothing has been
    /// established yet, so its failure is the caller's ordinary "herdr is not there" answer, with
    /// [`crate::Unavailable::NoSocket`] naming the path.
    pub async fn connect(
        client: HerdrClient,
        subscriptions: Vec<Subscription>,
    ) -> Result<(SessionSnapshot, Self)> {
        Self::connect_with(client, subscriptions, Backoff::default()).await
    }

    /// [`Self::connect`] with an explicit [`Backoff`].
    ///
    /// # Errors
    /// As [`Self::connect`].
    pub async fn connect_with(
        client: HerdrClient,
        subscriptions: Vec<Subscription>,
        backoff: Backoff,
    ) -> Result<(SessionSnapshot, Self)> {
        let (snapshot, events) = client.bootstrap(subscriptions.clone()).await?;
        Ok((
            snapshot,
            Self {
                client,
                subscriptions,
                backoff,
                events: Some(events),
                finished: false,
            },
        ))
    }

    /// The next item, or `None` once the attempt budget is spent.
    ///
    /// On a stream that ends for any reason — herdr restarted, the connection was cut, a line did
    /// not decode, the byte budget was spent — this re-runs [`HerdrClient::bootstrap`] behind
    /// [`Self::backoff`] and yields [`StreamItem::Snapshot`] before the events of the new stream.
    ///
    /// # Errors
    /// The last bootstrap failure, after [`Backoff::attempts`] consecutive failures. That `Err` is
    /// terminal: the stream is over and the next call answers `None`.
    pub async fn next(&mut self) -> Option<Result<StreamItem>> {
        if self.finished {
            return None;
        }
        // Why the reason the stream ended is carried, rather than re-derived: it is the answer
        // owed to the consumer when the attempt budget is zero and nothing is tried. Seeding the
        // retry loop with a fabricated failure instead would report a connection close that never
        // happened.
        let ended = match self.events.as_mut() {
            Some(events) => match events.next().await {
                Some(Ok(event)) => return Some(Ok(StreamItem::Event(event))),
                Some(Err(reason)) => {
                    tracing::debug!(%reason, "herdr event stream ended; re-bootstrapping");
                    reason
                }
                // A clean end **is** herdr closing the subscription connection — `Closed` is the
                // description of what happened here, not a placeholder for a missing one.
                None => HerdrError::Closed {
                    method: crate::stream::METHOD,
                },
            },
            // `events` is `Some` from construction and is cleared only on the line below, so this
            // arm is reachable only if that invariant is ever broken; "the stream is gone" is
            // still the truthful thing to say about it.
            None => HerdrError::Closed {
                method: crate::stream::METHOD,
            },
        };
        self.events = None;
        match self.rebootstrap(ended).await {
            Ok(snapshot) => Some(Ok(StreamItem::Snapshot(Box::new(snapshot)))),
            Err(last) => {
                self.finished = true;
                Some(Err(last))
            }
        }
    }

    /// Re-run `bootstrap` behind the backoff, up to [`Backoff::attempts`] times.
    ///
    /// The delay is taken **before** each attempt, including the first: a herdr that has just
    /// dropped the connection is mid-restart, and an immediate retry is the one that is certain to
    /// fail.
    ///
    /// `ended` is the failure that ended the stream, from [`Self::next`]. It is what a caller is
    /// told when [`Backoff::attempts`] is `0` — nothing was attempted, so the only true thing to
    /// report is why the stream stopped. Every attempt that does run replaces it, so what comes
    /// back is always a failure that actually happened, and always the **last** one.
    async fn rebootstrap(&mut self, ended: HerdrError) -> Result<SessionSnapshot> {
        let mut last = ended;
        for attempt in 0..self.backoff.attempts {
            tokio::time::sleep(self.backoff.delay(attempt)).await;
            match self.client.bootstrap(self.subscriptions.clone()).await {
                Ok((snapshot, events)) => {
                    self.events = Some(events);
                    return Ok(snapshot);
                }
                Err(reason) => {
                    tracing::debug!(
                        attempt = attempt + 1,
                        of = self.backoff.attempts,
                        %reason,
                        "herdr re-bootstrap failed"
                    );
                    last = reason;
                }
            }
        }
        Err(last)
    }
}
