//! [`HerdrEvents`] — the one connection herdr keeps open, read as a stream.
//!
//! ## Why this is not [`crate::transport::request`]
//!
//! Every other method on this socket is one line out and one line back, because
//! `handle_connection_with_stop` has no read loop (`tmp/herdr/src/api/server.rs:156-317`).
//! `events.subscribe` is the exception it dispatches to `stream_subscriptions` for
//! (`:229-250` → `:715-779`), and that function writes an acknowledgement and then **keeps
//! writing**:
//!
//! - line 1 is `{"id":"<your id>","result":{"type":"subscription_started"}}` (`:747-756`);
//! - every later line is an event with **no `id` field at all** —
//!   `tmp/herdr/src/api/subscriptions.rs:249-266` emits a bare `serde_json::Value`.
//!
//! herdr polls its subscriptions and flushes on a 100 ms tick (`CONNECTION_POLL_INTERVAL`,
//! `server.rs:28`, consumed at `:777`), so worst-case event latency is 100 ms and
//! `should_stop_connection` (`:812-821`) is the liveness check on the same tick.
//!
//! ## Draining is not optional
//!
//! herdr writes those events with a **blocking** `write_json_line` on a socket whose send timeout
//! it set to `STREAM_WRITE_TIMEOUT` = 5 s (`server.rs:164-166`, `:31`). A client that stops
//! reading therefore does not merely fall behind: herdr's write blocks, times out, and
//! `stream_subscriptions` returns — **the subscription is dropped**, silently from the client's
//! side. That is why [`crate::HerdrClient::bootstrap`] reads this stream *concurrently* with the
//! snapshot call rather than after it, and why [`HerdrEvents::next`] is the only shape offered:
//! there is no "peek later" that is safe.
//!
//! ## Lifecycle subscriptions do not replay
//!
//! *"Lifecycle subscriptions start when the request is accepted and do not replay events retained
//! before that point."* (`socket-api.mdx:817-819`), and the source agrees:
//! `stream_subscriptions` takes `event_hub.current_sequence()` as its floor **before** it builds
//! any subscription (`server.rs:723`). So the window between a snapshot and a later subscribe is
//! not a latency problem — those events are never sent. [`crate::HerdrClient::bootstrap`] exists
//! so that window cannot be opened.

use std::collections::VecDeque;

use tokio::io::{AsyncBufReadExt, BufReader};

use crate::error::{ApiError, ApiErrorCode, HerdrError, Result};
use crate::schema::events::Event;
use crate::schema::response::WireResponse;
use crate::transport::{LocalStream, MAX_RESPONSE_BYTES};

/// The wire method name every failure on this stream is attributed to.
pub(crate) const METHOD: &str = "events.subscribe";

/// How many bytes one subscription connection may carry before it is recycled — 128 MiB.
///
/// **A recycle threshold, not a memory bound.** The memory bound is [`MAX_RESPONSE_BYTES`], which
/// caps a single line; nothing accumulates across lines here, because each is decoded and handed
/// to the caller. This is the circuit breaker: a stream that has moved this much has either been
/// running for a very long time or is being fed something pathological, and re-bootstrapping costs
/// one `session.snapshot` and buys a guaranteed-consistent cache.
///
/// The figure is pi's, which applies the same multiple to the same per-line bound for the same
/// reason (`MAX_RPC_BYTES * 32`, `src/runs/shared/herdr-connection.ts:100` @v0.68.0). pi's
/// consumer learns about it through `onDisconnect`; here it arrives as a terminal
/// [`HerdrError::TooLarge`] on [`HerdrEvents::next`], and [`crate::ReconnectingEvents`] answers it
/// by re-running `bootstrap` — which is what makes it a recycle rather than a loss.
pub const MAX_STREAM_BYTES: u64 = 128 * 1024 * 1024;

/// A live `events.subscribe` stream.
///
/// Obtained from [`crate::HerdrClient::bootstrap`] (with a snapshot, in the order herdr requires)
/// or [`crate::HerdrClient::subscribe`] (on its own, for a consumer that keeps no cache).
///
/// Dropping it closes the connection, which is how herdr learns to stop
/// (`should_stop_connection`, `tmp/herdr/src/api/server.rs:812-821`).
///
/// **Every error is terminal.** [`Self::next`] yields at most one `Err` and then `None` for ever:
/// a malformed line, an unexpected error envelope, a byte bound or a closed socket all mean this
/// connection's state is no longer known, and continuing to read one would be guessing. Recovery
/// is a new subscription plus a fresh snapshot, which is [`crate::ReconnectingEvents`].
#[derive(Debug)]
pub struct HerdrEvents {
    reader: BufReader<LocalStream>,
    /// Lines already read but not yet handed out — everything that arrived while
    /// [`crate::HerdrClient::bootstrap`] was fetching the snapshot, in arrival order.
    buffered: VecDeque<Result<Event>>,
    /// A partially read line, kept across cancellations. See [`Self::fill`].
    partial: Vec<u8>,
    /// Bytes read on this connection so far, against [`MAX_STREAM_BYTES`].
    bytes: u64,
    /// Set once a terminal condition has been produced; `next` answers `None` from then on.
    finished: bool,
}

impl HerdrEvents {
    /// Wrap a connection whose `subscription_started` acknowledgement has already been read.
    pub(crate) fn new(reader: BufReader<LocalStream>, bytes: u64) -> Self {
        Self {
            reader,
            buffered: VecDeque::new(),
            partial: Vec::new(),
            bytes,
            finished: false,
        }
    }

    /// The next event, or `None` once the stream has ended.
    ///
    /// Buffered lines — the ones that arrived during a [`crate::HerdrClient::bootstrap`]'s
    /// snapshot — come first and in arrival order, then live ones. A caller cannot tell the two
    /// apart, which is the point: the sequence is exactly what herdr pushed.
    ///
    /// # Errors
    /// [`HerdrError::Closed`] when herdr closed the connection, [`HerdrError::Malformed`] for a
    /// line that is not a decodable event, [`HerdrError::Api`] for an error envelope pushed after
    /// the acknowledgement, [`HerdrError::TooLarge`] past [`MAX_RESPONSE_BYTES`] or
    /// [`MAX_STREAM_BYTES`], and [`HerdrError::Io`] otherwise. Each is the **last** thing this
    /// stream produces.
    pub async fn next(&mut self) -> Option<Result<Event>> {
        if let Some(buffered) = self.buffered.pop_front() {
            return Some(buffered);
        }
        if self.finished {
            return None;
        }
        self.read_next().await
    }

    /// Read one line and push whatever it decodes to onto [`Self::buffered`].
    ///
    /// This is what [`crate::HerdrClient::bootstrap`] races against `session.snapshot`, so it must
    /// be **cancel-safe**: the only `.await` is [`AsyncBufReadExt::fill_buf`], which is itself
    /// cancel-safe (a dropped `fill_buf` leaves the bytes in the `BufReader`), and every byte it
    /// returns is moved into [`Self::partial`] — a field, not a local — before the next await
    /// point. Dropping this future mid-line therefore loses nothing; the next call resumes on the
    /// same partial line.
    pub(crate) async fn buffer_one(&mut self) {
        if self.finished {
            return;
        }
        if let Some(item) = self.read_next().await {
            self.buffered.push_back(item);
        }
    }

    /// `true` once no further line will be produced.
    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    /// Bytes read on this connection, against [`MAX_STREAM_BYTES`].
    #[must_use]
    pub fn bytes_read(&self) -> u64 {
        self.bytes
    }

    /// Close the connection and stop the subscription.
    ///
    /// Equivalent to dropping the value; it exists so a caller can say so at a point it chooses
    /// rather than at the end of a scope.
    pub fn close(self) {
        drop(self);
    }

    /// Read and decode one line, marking the stream finished on any terminal condition.
    async fn read_next(&mut self) -> Option<Result<Event>> {
        let line = match self.fill().await {
            Ok(Some(line)) => line,
            Ok(None) => {
                self.finished = true;
                return Some(Err(HerdrError::Closed { method: METHOD }));
            }
            Err(error) => {
                self.finished = true;
                return Some(Err(error));
            }
        };
        let line = line.trim_end_matches(['\r', '\n']);

        // An error envelope after the acknowledgement. herdr's own stream loop writes only events
        // once it has acked (`tmp/herdr/src/api/server.rs:762-778`), so this is a herdr that has
        // changed or a socket that is not the one this client thinks it is — either way the
        // subscription's state is unknown. pi ends the stream on it too
        // (`src/runs/shared/herdr-connection.ts:100` @v0.68.0, the post-ack `record.error` arm).
        if let Ok(WireResponse::Error(failure)) = WireResponse::decode(line) {
            self.finished = true;
            return Some(Err(HerdrError::Api {
                method: METHOD,
                source: ApiError {
                    code: ApiErrorCode::from_wire(&failure.error.code),
                    message: failure.error.message,
                },
            }));
        }

        match Event::decode(line) {
            Ok(event) => Some(Ok(event)),
            Err(source) => {
                self.finished = true;
                Some(Err(HerdrError::Malformed {
                    method: METHOD,
                    source,
                }))
            }
        }
    }

    /// Accumulate bytes into [`Self::partial`] until a `\n`, then take the line.
    ///
    /// `Ok(None)` is EOF before any newline.
    async fn fill(&mut self) -> Result<Option<String>> {
        loop {
            let available = self.reader.fill_buf().await?;
            if available.is_empty() {
                return Ok(None);
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let taken = newline.map_or(available.len(), |index| index + 1);
            self.partial
                .extend_from_slice(available.get(..taken).unwrap_or_default());
            self.reader.consume(taken);
            self.bytes = self.bytes.saturating_add(taken as u64);

            if self.partial.len() > MAX_RESPONSE_BYTES {
                return Err(HerdrError::TooLarge {
                    method: METHOD,
                    limit: MAX_RESPONSE_BYTES,
                });
            }
            if self.bytes > MAX_STREAM_BYTES {
                return Err(HerdrError::TooLarge {
                    method: METHOD,
                    // `usize` on the error, `u64` on the counter: the counter outlives any one
                    // line, and a 32-bit target must still be able to count past 4 GiB of stream.
                    limit: usize::try_from(MAX_STREAM_BYTES).unwrap_or(usize::MAX),
                });
            }
            if newline.is_some() {
                let line = std::mem::take(&mut self.partial);
                return String::from_utf8(line).map(Some).map_err(|err| {
                    HerdrError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err))
                });
            }
        }
    }
}
