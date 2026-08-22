//! The finalizing event-stream contract (1:1 port of Pi `ai/src/utils/event-stream.ts`).
//!
//! Pi's `EventStream<T, R>` is a real object: `push(event)`, `end(result?)`, an awaitable
//! `result(): Promise<R>`, async iteration, and pluggable `isComplete`/`extractResult`. cyrup
//! standardizes async iteration on [`EventStream<T>`] (a `futures::Stream`, arch-00 §3.1);
//! this module adds the missing `result()`/`complete()` surface as the [`Finalizing`] trait
//! (arch-00 §3.1, realizing func-01 R-01-023/R-01-005) plus a push-driven [`FinalizingStream`] +
//! [`FinalizingSink`] that extensions can drive (Pi `createAssistantMessageEventStream`,
//! event-stream.ts:85-88; the `AssistantMessageEventStream` specialization lives in cyrup-provider
//! where `StreamEvent` is defined).
//!
//! **Terminal semantics (the cyrup resolution).** A terminal push — one satisfying `is_complete` —
//! delivers the event and then CLOSES the channel, so [`Finalizing::result`] resolves on the
//! terminal item and does NOT wait for [`FinalizingSink::end`]. This is the documented Pi contract
//! (`push` settles the `result()` promise the moment `isComplete(event)` holds,
//! event-stream.ts:21-36), and closing early loses nothing: `push` is already a no-op once a
//! terminal has fired, and a later `end(Some(v))` still overrides the captured value for any
//! `result()` that has not yet resolved.

use futures_core::Stream;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The single streaming primitive used across provider, agent, and tools (arch-00 §3.1).
///
/// Transport failures are delivered AS terminal items of `T` (never in `Err`); see arch-00 §3.1
/// and func-01 R-01-018.
pub type EventStream<T> = std::pin::Pin<Box<dyn futures_core::Stream<Item = T> + Send + 'static>>;

/// A [`Stream`] that additionally resolves to a final value once it terminates (arch-00 §3.1,
/// Pi `EventStream.result()`, event-stream.ts:64-66).
///
/// `result()` resolves to the final value when the terminal item fires (INCLUDING error/abort
/// terminals — it does NOT error), matching Pi's `result(): Promise<R>` which is keyed on
/// `isComplete` and yields `extractResult(event)` (event-stream.ts:21-27).
pub trait Finalizing<T, F>: Stream<Item = T> {
    /// Drive the stream to its terminal item and resolve the final value (never errors).
    fn result(self: Pin<&mut Self>) -> impl Future<Output = F> + Send;
}

/// Shared completion logic between a [`FinalizingSink`] and its [`FinalizingStream`] (Pi
/// `EventStream`'s `isComplete`/`extractResult`/`finalResult` closures, event-stream.ts:10-18).
struct Shared<T, F> {
    is_complete: Box<dyn Fn(&T) -> bool + Send + Sync>,
    extract: Box<dyn Fn(&T) -> F + Send + Sync>,
    fallback: Box<dyn Fn() -> F + Send + Sync>,
    final_value: Mutex<Option<F>>,
}

/// The producer half (Pi `EventStream.push`/`end`, event-stream.ts:21-48). Drives a push-based
/// producer (e.g. an extension authoring a stream) into the consumer [`FinalizingStream`].
pub struct FinalizingSink<T, F> {
    tx: Option<mpsc::UnboundedSender<T>>,
    shared: Arc<Shared<T, F>>,
    done: bool,
}

impl<T, F> FinalizingSink<T, F> {
    /// Push one event (Pi `push`, event-stream.ts:21-36). When the event satisfies `is_complete`,
    /// the final value is captured via `extract`, the event is delivered, and the channel is then
    /// closed so [`Finalizing::result`] resolves on the terminal item without requiring
    /// [`end`](Self::end) (see the module doc's terminal-semantics note). No-op after the stream is
    /// complete or ended.
    pub fn push(&mut self, event: T) {
        if self.done {
            return;
        }
        if (self.shared.is_complete)(&event) {
            self.done = true;
            // `extract` is caller-supplied: run it BEFORE taking the lock, so a panicking closure
            // unwinds without poisoning `final_value` (a poisoned slot used to make a subsequent
            // `end(Some(v))` and `result()` fall through to `fallback` with no error and no log).
            let value = (self.shared.extract)(&event);
            // A poisoned lock still guards a usable slot: recover it rather than drop the value.
            *self.shared.final_value.lock().unwrap_or_else(PoisonError::into_inner) = Some(value);
            if let Some(tx) = self.tx.take() {
                // Delivery failure means the consumer dropped the stream — stop producing.
                let _ = tx.send(event);
            }
            // `tx` is dropped here: the receiver stream ends after this terminal event, which is
            // what lets `result()` resolve on the terminal instead of blocking on `end`.
            return;
        }
        if let Some(tx) = &self.tx {
            // Delivery failure means the consumer dropped the stream — stop producing.
            let _ = tx.send(event);
        }
    }

    /// End the stream (Pi `end`, event-stream.ts:38-48). Closes the consumer iteration; an
    /// optional pre-computed final result overrides the extracted one.
    pub fn end(&mut self, result: Option<F>) {
        self.done = true;
        if let Some(r) = result {
            // Recover a poisoned lock instead of silently discarding the caller's explicit final
            // value (a `push` whose `extract` panicked must not defeat `end(Some(v))`).
            *self.shared.final_value.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
        }
        self.tx = None; // dropping the sender closes the receiver stream
    }

    /// `true` once a terminal event has been pushed or [`end`](Self::end) called.
    pub fn is_done(&self) -> bool {
        self.done
    }
}

/// Deliberately unconditional in `T`/`F`: `Shared` holds boxed closures that can never be `Debug`,
/// and adding `T: Debug`/`F: Debug` bounds would strip `Debug` from downstream aliases (e.g. the
/// `StreamEvent`/`AgentEvent` specializations). Reports only the producer state, never the events.
impl<T, F> fmt::Debug for FinalizingSink<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinalizingSink")
            .field("open", &self.tx.is_some())
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

/// The consumer half: a [`Stream`] of `T` that also implements [`Finalizing<T, F>`].
pub struct FinalizingStream<T, F> {
    rx: UnboundedReceiverStream<T>,
    shared: Arc<Shared<T, F>>,
}

/// Unconditional in `T`/`F` for the same reason as [`FinalizingSink`]'s impl: no `Debug` bounds,
/// so downstream aliases over non-`Debug` event/result types keep their own `Debug`.
impl<T, F> fmt::Debug for FinalizingStream<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinalizingStream").finish_non_exhaustive()
    }
}

impl<T, F> Stream for FinalizingStream<T, F> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

impl<T, F> Finalizing<T, F> for FinalizingStream<T, F>
where
    T: Send,
    F: Send,
{
    fn result(self: Pin<&mut Self>) -> impl Future<Output = F> + Send {
        // FinalizingStream is Unpin (all fields are), so it is safe to project to `&mut Self`.
        let this = self.get_mut();
        async move {
            // Drain remaining events; the final value is captured on push.
            loop {
                let next = std::future::poll_fn(|cx| Pin::new(&mut this.rx).poll_next(cx)).await;
                if next.is_none() {
                    break;
                }
            }
            // Same poison recovery as `push`/`end`: a captured value survives a panicking closure.
            let taken =
                this.shared.final_value.lock().unwrap_or_else(PoisonError::into_inner).take();
            taken.unwrap_or_else(|| (this.shared.fallback)())
        }
    }
}

/// Create a push-driven finalizing stream (Pi `new EventStream(isComplete, extractResult)`,
/// event-stream.ts:13-19, + `createAssistantMessageEventStream`, 85-88).
///
/// - `is_complete` — `true` for a terminal event (Pi `isComplete`).
/// - `extract` — the final value for a terminal event (Pi `extractResult`).
/// - `fallback` — the final value if the stream ends WITHOUT a terminal (Pi's `result()` would
///   hang forever; cyrup resolves a sensible terminal-less fallback so `result()` never blocks,
///   consistent with `collect_message`'s no-panic synthesis). With a terminal, `result()` resolves
///   on the terminal item itself — `end` is not required (see the module doc).
pub fn finalizing_channel<T, F>(
    is_complete: impl Fn(&T) -> bool + Send + Sync + 'static,
    extract: impl Fn(&T) -> F + Send + Sync + 'static,
    fallback: impl Fn() -> F + Send + Sync + 'static,
) -> (FinalizingSink<T, F>, FinalizingStream<T, F>) {
    let shared = Arc::new(Shared {
        is_complete: Box::new(is_complete),
        extract: Box::new(extract),
        fallback: Box::new(fallback),
        final_value: Mutex::new(None),
    });
    let (tx, rx) = mpsc::unbounded_channel();
    let sink = FinalizingSink { tx: Some(tx), shared: shared.clone(), done: false };
    let stream = FinalizingStream { rx: UnboundedReceiverStream::new(rx), shared };
    (sink, stream)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::pin::pin;
    use std::time::Duration;

    fn channel() -> (FinalizingSink<i32, i32>, FinalizingStream<i32, i32>) {
        finalizing_channel(|e| *e < 0, |e| *e, || i32::MIN)
    }

    #[tokio::test]
    async fn result_resolves_to_terminal_value() {
        let (mut sink, stream) = channel();
        sink.push(1);
        sink.push(2);
        sink.push(-7); // terminal
        sink.end(None);
        let mut s = pin!(stream);
        let r = s.as_mut().result().await;
        assert_eq!(r, -7);
    }

    #[tokio::test]
    async fn result_uses_fallback_without_terminal() {
        let (mut sink, stream) = channel();
        sink.push(1);
        sink.end(None);
        let mut s = pin!(stream);
        assert_eq!(s.as_mut().result().await, i32::MIN);
    }

    /// AC: a terminal push alone must settle `result()` — the sink is deliberately still alive and
    /// un-ended, and the bounded timeout fails the test rather than hanging the harness.
    #[tokio::test]
    async fn result_resolves_on_terminal_without_end() {
        let (mut sink, stream) = channel();
        sink.push(1);
        sink.push(-7); // terminal — no `end()` call, and `sink` stays in scope below
        let mut s = pin!(stream);
        let r = tokio::time::timeout(Duration::from_millis(300), s.as_mut().result())
            .await
            .expect("result() must resolve on the terminal item, not wait for end()");
        assert_eq!(r, -7);
        assert!(sink.is_done());
    }

    /// AC: `extract` runs outside the lock, so a panicking closure cannot poison `final_value` into
    /// swallowing a later `end(Some(99))` (this previously yielded the `i32::MIN` fallback).
    #[tokio::test]
    #[allow(clippy::panic)] // the `extract` closure below panics on purpose
    async fn explicit_end_survives_a_panicking_extract() {
        let (mut sink, stream) =
            finalizing_channel(|e: &i32| *e < 0, |_: &i32| panic!("boom"), || i32::MIN);
        let pushed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink.push(-1)));
        assert!(pushed.is_err(), "the deliberately-panicking extract must unwind");
        sink.end(Some(99));
        let mut s = pin!(stream);
        let r = tokio::time::timeout(Duration::from_millis(300), s.as_mut().result())
            .await
            .expect("result() must not hang after a panicking extract");
        assert_eq!(r, 99);
    }

    #[tokio::test]
    async fn iterates_in_push_order() {
        use tokio_stream::StreamExt;
        let (mut sink, mut stream) = channel();
        sink.push(10);
        sink.push(20);
        sink.end(None);
        let mut got = Vec::new();
        while let Some(v) = stream.next().await {
            got.push(v);
        }
        assert_eq!(got, vec![10, 20]);
    }
}
