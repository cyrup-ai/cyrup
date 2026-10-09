//! Response timing: [`ResponseTimer`] sets [`AssistantMessage::duration_ms`] on the final message
//! of one assistant response — pi `AssistantMessageEventStream`'s `#time`
//! (`packages/ai/src/utils/event-stream.ts` @v1.1.0, commit 36a686ee8).
//!
//! pi times every response in ONE class: each api implementation creates an
//! `AssistantMessageEventStream`, which captures `Date.now()` and `performance.now()` when it is
//! constructed, and stamps `durationMs` on the first `done`/`error` event (or the result handed to
//! `end()`). A stream that forwards another stream's events stamps nothing, because the inner stream
//! got there first. cyrup has no single stream class, so the same rule is applied wherever a stream
//! of one response is opened:
//!
//! - [`crate::api::EventSink`] — every wire api implementation and the [`crate::Models`] forwarders
//!   push through one, so its timer is pi's per-stream timer;
//! - [`timed`] — wraps a stream produced elsewhere (a faux, local or extension provider), timed from
//!   the moment it is wrapped;
//! - the agent loop, which also times the messages it settles itself (an abort, a stream that ended
//!   without a terminal), as pi's stream times the `error` event an abort produces.
//!
//! Layering is safe for the reason pi's forwarding is: a message that already carries a duration is
//! never re-timed, so the innermost timer wins.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Instant;

use cyrup_core::{AssistantMessage, EventStream};
use futures::Stream;

use crate::stream::StreamEvent;
use crate::utils::provider_plumbing::now_millis;

/// The clock of one assistant response. See the module docs.
#[derive(Debug)]
pub struct ResponseTimer {
    /// Wall clock at the start (pi `#startedAt = Date.now()`).
    started_at: i64,
    /// Monotonic clock at the start (pi `#startedAtMonotonic = performance.now()`).
    started: Instant,
    /// Set once a terminal has gone through (pi `EventStream.done`): later terminals and the
    /// response's `end()` are not timed again.
    done: AtomicBool,
}

impl ResponseTimer {
    /// Start timing now.
    #[must_use]
    pub fn start() -> Self {
        Self {
            started_at: now_millis(),
            started: Instant::now(),
            done: AtomicBool::new(false),
        }
    }

    /// Wall-clock start of this response, in Unix milliseconds (pi `startedAt`): the `timestamp` of
    /// every message this response produces — pi's api implementations seed `output.timestamp =
    /// Date.now()` before the request goes out and never move it, and the v1.1.0 type documents it
    /// as *"Unix timestamp in milliseconds when the request started."*
    #[must_use]
    pub const fn started_at(&self) -> i64 {
        self.started_at
    }

    /// Time `message` as this response's final message, unless it already carries a duration or
    /// started before this response did (pi `#time`, without the `done` check — see
    /// [`Self::time_terminal`]). Returns whether it was stamped.
    ///
    /// A message with `timestamp: 0` predates every response, so it is left untimed — which is
    /// right for a message that was MADE before the request: the faux provider's scripted replies
    /// carry `0` where pi's carry the time the test built them (a documented CYRUP-DELTA in
    /// `faux.rs`), and pi leaves those untimed. A message this response CREATED and left unstamped
    /// goes through [`Self::time_created`] instead.
    pub fn time(&self, message: &mut AssistantMessage) -> bool {
        if message.duration_ms.is_some() || message.timestamp < self.started_at {
            return false;
        }
        message.duration_ms = Some(self.elapsed_ms());
        true
    }

    /// [`Self::time`] for a message this response created itself: one left at `timestamp: 0` —
    /// cyrup's "not stamped" marker, which [`AssistantMessage::errored`] leaves where pi's messages
    /// always carry `Date.now()` — first takes this response's start, which is what pi's
    /// `lazyStream` does for a setup failure (`createSetupErrorMessage(model, error,
    /// outer.startedAt)`, `packages/ai/src/api/lazy.ts` @v1.1.0).
    pub fn time_created(&self, message: &mut AssistantMessage) -> bool {
        if message.duration_ms.is_none() && message.timestamp == 0 {
            message.timestamp = self.started_at;
        }
        self.time(message)
    }

    /// [`Self::time`] for the FIRST terminal this response delivers; a no-op for every other event
    /// and for any terminal after the first (pi `if (this.done || …) return`).
    pub fn time_terminal(&self, event: &mut StreamEvent) {
        self.terminal(event, Self::time);
    }

    /// [`Self::time_terminal`] for a stream whose terminals this response CREATED — a provider
    /// channel, where an unstamped message is one the api implementation just built (see
    /// [`Self::time_created`]).
    pub fn time_created_terminal(&self, event: &mut StreamEvent) {
        self.terminal(event, Self::time_created);
    }

    fn terminal(&self, event: &mut StreamEvent, time: fn(&Self, &mut AssistantMessage) -> bool) {
        let message = match event {
            StreamEvent::Done { message, .. } => message,
            StreamEvent::Error { error, .. } => error,
            _ => return,
        };
        if self.done.swap(true, Ordering::SeqCst) {
            return;
        }
        if message.duration_ms.is_some() {
            return;
        }
        // Clones only if another holder still shares the message.
        time(self, Arc::make_mut(message));
    }

    /// Milliseconds since the start, rounded as pi rounds (`Math.max(0, Math.round(...))`).
    fn elapsed_ms(&self) -> u64 {
        let ms = (self.started.elapsed().as_secs_f64() * 1000.0).round();
        // `as` saturates a float into the integer range, so a negative or absurd value cannot wrap.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ms = ms.max(0.0) as u64;
        ms
    }
}

/// Time the response `inner` produces, from now: its first terminal gets `durationMs` unless an
/// inner timer already set one.
#[must_use]
pub fn timed(inner: EventStream<StreamEvent>) -> EventStream<StreamEvent> {
    Box::pin(Timed {
        inner,
        timer: ResponseTimer::start(),
    })
}

struct Timed {
    inner: EventStream<StreamEvent>,
    timer: ResponseTimer,
}

impl Stream for Timed {
    type Item = StreamEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<StreamEvent>> {
        let this = &mut *self;
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(mut event)) => {
                this.timer.time_terminal(&mut event);
                Poll::Ready(Some(event))
            }
            other => other,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    //! pi's own cases, `packages/ai/test/event-stream.test.ts` @v1.1.0 (commit 36a686ee8), on each
    //! cyrup surface that plays the part of `AssistantMessageEventStream`.
    use super::*;
    use crate::api::channel;
    use crate::stream::{DoneReason, ErrorReason, create_assistant_message_event_stream};
    use cyrup_core::{Finalizing, StopReason, Usage};
    use futures::StreamExt;
    use std::time::Duration;

    fn message(timestamp: i64, duration_ms: Option<u64>) -> AssistantMessage {
        AssistantMessage {
            content: Vec::new(),
            provider: "openai".into(),
            model: "m".into(),
            api: "openai-responses".into(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            thinking_level: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp,
            duration_ms,
        }
    }

    fn done(m: AssistantMessage) -> StreamEvent {
        StreamEvent::Done {
            reason: DoneReason::Stop,
            message: Arc::new(m),
        }
    }

    fn error(m: AssistantMessage) -> StreamEvent {
        StreamEvent::Error {
            reason: ErrorReason::Error,
            error: Arc::new(AssistantMessage {
                stop_reason: StopReason::Error,
                ..m
            }),
        }
    }

    fn duration_of(event: &StreamEvent) -> Option<u64> {
        event.terminal_message().and_then(|m| m.duration_ms)
    }

    /// pi: "sets durationMs on the final done or error message of a response it saw start".
    #[tokio::test]
    async fn the_final_done_or_error_message_of_a_response_is_timed() {
        let (sink, mut rx) = channel(4);
        let answer = message(now_millis(), None);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(sink.send(done(answer)).await);
        let got = rx.recv().await.expect("the terminal");
        assert!(duration_of(&got).expect("timed") >= 15, "{got:?}");

        let (sink, mut rx) = channel(4);
        assert!(sink.send(error(message(now_millis(), None))).await);
        assert!(duration_of(&rx.recv().await.expect("the terminal")).is_some());

        // The extension-authored stream, through `end(result)` with no terminal pushed.
        let (mut sink, mut stream) = create_assistant_message_event_stream();
        sink.end(Some(message(now_millis(), None)));
        let result = std::pin::Pin::new(&mut stream).result().await;
        assert!(result.duration_ms.is_some(), "{result:?}");
    }

    /// pi: "keeps an existing duration, so a forwarding stream keeps the inner measurement".
    #[tokio::test]
    async fn an_existing_duration_is_kept_so_a_forwarder_keeps_the_inner_measurement() {
        let (outer, mut outer_rx) = channel(4);
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (inner, mut inner_rx) = channel(4);
        assert!(inner.send(done(message(now_millis(), None))).await);
        let measured = inner_rx.recv().await.expect("inner terminal");
        let inner_ms = duration_of(&measured).expect("the inner timer measured it");
        assert!(outer.send(measured).await);
        let forwarded = outer_rx.recv().await.expect("outer terminal");
        assert_eq!(duration_of(&forwarded), Some(inner_ms));
        assert!(
            inner_ms < 20,
            "the inner stream started after the sleep: {inner_ms}"
        );

        let (preset, mut rx) = channel(4);
        assert!(preset.send(done(message(now_millis(), Some(1234)))).await);
        assert_eq!(duration_of(&rx.recv().await.unwrap()), Some(1234));

        // `timed` over an already-timed stream is the same forwarder.
        let inner: EventStream<StreamEvent> = Box::pin(futures::stream::iter(vec![done(message(
            now_millis(),
            Some(7),
        ))]));
        let got = timed(inner).next().await.unwrap();
        assert_eq!(duration_of(&got), Some(7));
    }

    /// pi: "leaves a message untimed when it started before the stream, such as a fetched deferred
    /// result".
    #[tokio::test]
    async fn a_message_that_started_before_the_stream_is_left_untimed() {
        let (sink, mut rx) = channel(4);
        assert!(sink.send(done(message(now_millis() - 60_000, None))).await);
        assert_eq!(duration_of(&rx.recv().await.unwrap()), None);
    }

    /// pi: "does not time a message pushed after the stream completed".
    #[tokio::test]
    async fn a_terminal_after_the_first_is_not_timed() {
        let (sink, mut rx) = channel(4);
        assert!(sink.send(done(message(now_millis(), None))).await);
        assert!(sink.send(done(message(now_millis(), None))).await);
        assert!(duration_of(&rx.recv().await.unwrap()).is_some());
        assert_eq!(duration_of(&rx.recv().await.unwrap()), None);
    }

    /// cyrup's own marker: `AssistantMessage::errored` leaves `timestamp` at 0 where pi always has
    /// `Date.now()`. Such a message takes the response's start — pi's `lazyStream` setup-error path
    /// (`createSetupErrorMessage(model, error, outer.startedAt)`) — and is then timed, rather than
    /// falling to the "started before the stream" rule and staying untimed.
    #[tokio::test]
    async fn an_unstamped_error_takes_the_responses_start_and_is_timed() {
        let (sink, mut rx) = channel(4);
        let started_at = sink.started_at();
        let failed = AssistantMessage::errored(
            "openai".into(),
            "m",
            None,
            StopReason::Error,
            "no credential",
        );
        assert_eq!(failed.timestamp, 0);
        assert!(sink.send(StreamEvent::terminal(failed)).await);
        let got = rx.recv().await.unwrap();
        let m = got.terminal_message().unwrap();
        assert_eq!(m.timestamp, started_at);
        assert!(m.duration_ms.is_some());
    }

    /// The extension-authored stream times a terminal it is PUSHED, as pi's `push` override does
    /// (`utils/event-stream.ts` @v1.1.0): the event the consumer reads and the `result()` agree.
    #[tokio::test]
    async fn the_extension_authored_stream_times_a_pushed_terminal() {
        let (mut sink, mut stream) = create_assistant_message_event_stream();
        tokio::time::sleep(Duration::from_millis(5)).await;
        sink.push(done(message(now_millis(), None)));
        sink.end(None);
        let ev = stream.next().await.expect("the terminal");
        let took = duration_of(&ev).expect("timed on push");
        let result = std::pin::Pin::new(&mut stream).result().await;
        assert_eq!(result.duration_ms, Some(took));
    }

    /// A direct caller (the agent loop times the message it settled) gets the same rule as the
    /// stream: a message that already carries a duration keeps it.
    #[test]
    fn time_keeps_an_existing_duration() {
        let timer = ResponseTimer::start();
        let mut preset = message(now_millis(), Some(1234));
        assert!(!timer.time(&mut preset));
        assert_eq!(preset.duration_ms, Some(1234));
        assert!(!timer.time_created(&mut preset));
        assert_eq!(preset.duration_ms, Some(1234));
    }

    /// A message MADE before the response (`timestamp: 0`, the faux provider's scripted replies)
    /// stays untimed through a forwarding wrapper, as pi's scripted replies, built before the
    /// stream, do; only a provider channel treats `0` as "created now".
    #[tokio::test]
    async fn a_message_made_before_the_response_stays_untimed_through_a_wrapper() {
        let scripted: EventStream<StreamEvent> =
            Box::pin(futures::stream::iter(vec![done(message(0, None))]));
        let got = timed(scripted).next().await.unwrap();
        let m = got.terminal_message().unwrap();
        assert_eq!((m.timestamp, m.duration_ms), (0, None));
    }

    /// Non-terminal events pass untouched and do not count as the first terminal.
    #[tokio::test]
    async fn a_partial_is_not_timed_and_does_not_end_the_response() {
        let (sink, mut rx) = channel(4);
        let partial = Arc::new(message(now_millis(), None));
        assert!(sink.send(StreamEvent::Start { partial }).await);
        assert!(sink.send(done(message(now_millis(), None))).await);
        let start = rx.recv().await.unwrap();
        assert!(start.partial().is_some_and(|p| p.duration_ms.is_none()));
        assert!(duration_of(&rx.recv().await.unwrap()).is_some());
    }
}
