//! Cancellation pulled *into* a blocking `std::io::Read` consumer.
//!
//! A `tokio::task::spawn_blocking` task owns an OS thread and **cannot be aborted from outside** —
//! dropping its `JoinHandle` does not stop the thread. So for an in-process blocking reader loop
//! (`grep`'s `grep_searcher::Searcher::search_reader`, a streaming whole-file read) the only way
//! out is for the work itself to fail. `CancelReader` turns the run's [`CancelToken`] into
//! exactly that failure, and `Cancelled` is the payload the consumer matches on to tell "the
//! token fired mid-file" apart from a genuine I/O error.

use cyrup_core::CancelToken;

/// The payload attached to the `io::Error` that a cancelled `CancelReader` read — or any other
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

/// Buffer size for [`read_to_end_cancellable`]: the granularity at which the token is observed.
///
/// Deliberately NOT `Read::read_to_end`. Its `default_read_to_end` grows the destination
/// geometrically and issues correspondingly huge single `read` calls, so the token would be
/// consulted a handful of times over a multi-GB file. A fixed buffer makes abort latency a function
/// of device throughput, not of file size. 64 KiB also matches `grep_searcher`'s
/// `DEFAULT_BUFFER_CAPACITY` (grep-searcher-0.1.16 `src/line_buffer.rs:6`), so both consumers of
/// this module abort on the same beat.
const CHUNK: usize = 64 * 1024;

/// Drain `reader` into a `Vec`, observing `cancel` every [`CHUNK`] bytes and refusing to hold more
/// than `limit` bytes.
///
/// Returns an `Err` carrying `Cancelled` (test it with `Cancelled::is`) when the token fires
/// mid-transfer; the partial buffer is dropped, exactly as Pi discards a partially-read file when
/// the promise rejects.
///
/// [CYRUP-DELTA] The `limit` has no Pi counterpart on the drain itself: Node's `readFile` sizes its
/// buffer from `fstat` and stops at that size (and refuses a regular file over 2 GiB with
/// `ERR_FS_FILE_TOO_LARGE`), so a file is bounded by what `stat` reported. A reader whose length is
/// not known up front (a pipe, `/dev/zero`, a `/proc` file, a file another process keeps appending
/// to) has no such bound, and this loop would otherwise grow the `Vec` until the host runs out of
/// memory while only the tool's own wall clock stops it. The loop therefore fails with
/// [`std::io::ErrorKind::FileTooLarge`] as soon as one more byte than `limit` has arrived. A reader
/// that delivers exactly `limit` bytes and then ends is accepted.
pub(crate) fn read_to_end_cancellable<R: std::io::Read>(
    reader: R,
    cancel: &CancelToken,
    limit: usize,
) -> std::io::Result<Vec<u8>> {
    use std::io::Read as _;

    let mut src = CancelReader::new(reader, cancel.clone());
    let mut out: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match src.read(&mut buf) {
            Ok(0) => return Ok(out),
            // `Read` is a safe trait, so a broken implementation CAN claim more bytes than the
            // buffer holds. `get` refuses to index blindly (crate-wide `clippy::indexing_slicing`)
            // and turns that contract violation into an error instead of a panic.
            Ok(n) => match buf.get(..n) {
                Some(chunk) => {
                    // Checked BEFORE the copy, so the buffer never holds more than `limit`.
                    if out.len().saturating_add(chunk.len()) > limit {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::FileTooLarge,
                            format!("more than {limit} bytes"),
                        ));
                    }
                    out.extend_from_slice(chunk);
                }
                None => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "read reported more bytes than the buffer holds",
                    ));
                }
            },
            // A REAL `EINTR`, not our cancel — the marker uses `ErrorKind::Other` precisely so this
            // retry cannot swallow it. The next iteration re-tests the token first.
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::io::Read as _;

    /// A reader far longer than any `limit` below. Finite on purpose: it stands in for `/dev/zero`,
    /// and a test that really was endless would take the machine down with it when the check it
    /// pins is the thing that broke.
    fn endless() -> impl std::io::Read {
        std::io::repeat(0).take(64 * CHUNK as u64)
    }

    #[test]
    fn a_reader_that_never_ends_stops_at_the_limit() {
        let err = read_to_end_cancellable(endless(), &CancelToken::new(), 3 * CHUNK + 5)
            .expect_err("a reader past the limit must not be drained");
        assert_eq!(err.kind(), std::io::ErrorKind::FileTooLarge, "{err}");
    }

    #[test]
    fn a_reader_of_exactly_the_limit_is_accepted() {
        let limit = 2 * CHUNK + 7;
        let bytes = read_to_end_cancellable(
            std::io::repeat(0).take(limit as u64),
            &CancelToken::new(),
            limit,
        )
        .unwrap();
        assert_eq!(bytes.len(), limit);
    }

    #[test]
    fn one_byte_over_the_limit_is_refused() {
        let limit = 2 * CHUNK + 7;
        let err = read_to_end_cancellable(
            std::io::repeat(0).take(limit as u64 + 1),
            &CancelToken::new(),
            limit,
        )
        .expect_err("limit + 1 bytes must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::FileTooLarge, "{err}");
    }

    #[test]
    fn a_fired_token_still_reports_cancelled_rather_than_too_large() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let err = read_to_end_cancellable(endless(), &cancel, 10).expect_err("cancelled");
        assert!(Cancelled::is(&err), "{err}");
    }
}
