//! Cancellation pulled *into* a blocking `std::io::Read` consumer.
//!
//! A `tokio::task::spawn_blocking` task owns an OS thread and **cannot be aborted from outside** —
//! dropping its `JoinHandle` does not stop the thread. So for an in-process blocking reader loop
//! (`grep`'s `grep_searcher::Searcher::search_reader`, a streaming whole-file read) the only way
//! out is for the work itself to fail. [`CancelReader`] turns the run's [`CancelToken`] into
//! exactly that failure, and [`Cancelled`] is the payload the consumer matches on to tell "the
//! token fired mid-file" apart from a genuine I/O error.

use cyrup_core::CancelToken;

/// The payload attached to the `io::Error` that a cancelled [`CancelReader`] read — or any other
/// cancelled callback that opts into the same marker — returns, so the caller can tell "the token
/// fired mid-file" apart from a genuine read failure. That distinction is load-bearing: a read
/// failure must stay a SILENT SKIP (rg emits no match events for a file it cannot read), while a
/// cancel must become `error::aborted`.
///
/// The kind is deliberately [`std::io::ErrorKind::Other`] and NOT `Interrupted`.
/// `grep_searcher::Searcher::search_reader` wraps the reader in `encoding_rs_io`'s
/// `DecodeReaderBytes`, whose BOM sniff goes through `util::read_full`, and that helper RETRIES on
/// `ErrorKind::Interrupted` (`Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}` inside its
/// `while !buf.is_empty()` loop, encoding_rs_io-0.1.7 `src/util.rs:230-241`). An `Interrupted`
/// sentinel would spin forever on the first read of every candidate instead of aborting it.
#[derive(Debug)]
pub(crate) struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Same wording as `error::aborted`, so a stray leak reads identically to Pi's rejection.
        f.write_str("Operation aborted")
    }
}

impl std::error::Error for Cancelled {}

impl Cancelled {
    /// The `io::Error` carrying this marker. `grep_searcher`'s `SinkError for io::Error` forwards
    /// reader errors verbatim (`fn error_io(err: io::Error) -> io::Error { err }`,
    /// grep-searcher-0.1.16 `src/sink.rs:47-49`), so the boxed payload survives the trip out of
    /// `search_reader` unchanged.
    pub(crate) fn err() -> std::io::Error {
        std::io::Error::other(Cancelled)
    }

    /// Recover the marker from whatever the blocking consumer returned.
    pub(crate) fn is(e: &std::io::Error) -> bool {
        e.get_ref().is_some_and(|src| src.is::<Cancelled>())
    }
}

/// A [`std::io::Read`] adapter that turns the run's [`CancelToken`] into an error the moment it
/// fires, so a cancel is observed DURING one file's read rather than at the next file boundary.
///
/// For `grep` this is the in-process stand-in for Pi's `onAbort` → `stopChild()` listener
/// (grep.ts:240-250): Pi kills the ripgrep child, which ends the search at once. cyrup has no child
/// to kill, and a `tokio::task::spawn_blocking` task cannot be aborted from outside, so the abort
/// has to be PULLED IN by the one thing the blocking `search_reader` call keeps asking us for —
/// bytes.
///
/// Granularity: `grep_searcher`'s `LineBuffer::fill` calls `rdr.read` once per buffer refill and
/// `?`-propagates the error without retrying (grep-searcher-0.1.16 `src/line_buffer.rs:406-418`),
/// and `DEFAULT_BUFFER_CAPACITY` is 64 KiB (`src/line_buffer.rs:6`). So the worst case after a
/// cancel is one 64 KiB chunk plus the regex scan over it, instead of one whole file.
pub(crate) struct CancelReader<R> {
    inner: R,
    cancel: CancelToken,
}

impl<R> CancelReader<R> {
    pub(crate) fn new(inner: R, cancel: CancelToken) -> Self {
        Self { inner, cancel }
    }
}

impl<R: std::io::Read> std::io::Read for CancelReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Before: never start another chunk once the token has fired.
        if self.cancel.is_cancelled() {
            return Err(Cancelled::err());
        }
        let n = self.inner.read(buf)?;
        // After: a cancel that landed WHILE this read was parked — a slow disk, a network-backed
        // `FsOps`, a `read_stream` that is not a plain `File` — is observed now rather than one
        // chunk later. Dropping the bytes just read is sound because the read is being abandoned:
        // the caller returns `error::aborted()` and no partial result is ever emitted.
        if self.cancel.is_cancelled() {
            return Err(Cancelled::err());
        }
        Ok(n)
    }
}
