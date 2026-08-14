//! `writeRawStdout` / `flushRawStdout` — the retrying protocol-stream writer (Pi
//! `core/output-guard.ts:9,20-43,85-103` @v0.84.1). TOOL-037.
//!
//! Pi's protocol writers never call `process.stdout.write` directly; every PRINT/JSON/RPC line goes
//! through `writeRawStdout` (print-mode.ts:110/115/141, rpc-mode.ts:60), whose per-chunk worker is
//! `writeRawStdoutChunk` (`output-guard.ts:20-43`):
//!
//! ```text
//! async function writeRawStdoutChunk(text: string): Promise<void> {
//!     while (true) {
//!         try { await new Promise((resolve, reject) => { getRawStdoutWrite()(text, cb) }); return; }
//!         catch (error) {
//!             const code = (error as Error & { code?: unknown }).code;
//!             if (code !== "ENOBUFS" && code !== "EAGAIN" && code !== "EWOULDBLOCK") throw error;
//!             await new Promise((resolve) => setTimeout(resolve, RAW_STDOUT_RETRY_DELAY_MS));
//!         }
//!     }
//! }
//! ```
//!
//! The retry is not defensive polish: when stdout is a **pipe that the OS or the reader put into
//! non-blocking mode** — which is exactly the shape a supervisor, a CI runner or another process
//! driving `--mode rpc` produces — a write returns `EAGAIN`/`EWOULDBLOCK` instead of blocking, and
//! `ENOBUFS` when the socket buffer is momentarily full. Treating those as fatal drops protocol
//! lines silently, or (in Rust) aborts the mode with an `io::Error` on a condition that is
//! transient by definition. Pi loops at a fixed 10 ms until the write lands and rethrows anything
//! else.
//!
//! ## Why this is a `Write`-generic helper rather than a process-global writer
//!
//! Pi can key on the global `process.stdout` because its takeover swap is user-space
//! (`output-guard.ts:45-70`). cyrup's protocol writers take an **injected** `std::io::Write` sink
//! that `main` binds to real stdout and the tests bind to a `Vec<u8>` — the same discipline pi's
//! `writeRawStdout` provides, achieved at the type level. So the retry belongs on the write itself,
//! and applies to whatever sink the host injected.
//!
//! ## CYRUP-DELTA — the fire-and-forget tail
//!
//! Pi's `writeRawStdout` chains onto a promise tail and does NOT await it (`:85-93`), which is why
//! it also needs `waitForRawStdoutBackpressure` (`:95-103`) to drain that tail and
//! `flushRawStdout` (`:105-108`) to await both. cyrup writes synchronously through the injected
//! sink and awaits each line, so the tail — and therefore the backpressure drain — has nothing to
//! model: the write *is* the await. [`flush_raw_stdout`] is the remaining half, pi's terminal
//! `writeRawStdoutChunk("")`, and it carries the same retry.

use std::io::{self, Write};
use std::time::Duration;

/// Pi `RAW_STDOUT_RETRY_DELAY_MS = 10` (`core/output-guard.ts:9` @v0.84.1).
pub const RAW_STDOUT_RETRY_DELAY_MS: u64 = 10;

/// Whether a write error is one of pi's three retryable codes — `ENOBUFS`, `EAGAIN`, `EWOULDBLOCK`
/// (`output-guard.ts:38`).
///
/// `EAGAIN` and `EWOULDBLOCK` are the same errno on every platform cyrup targets and Rust maps both
/// to [`io::ErrorKind::WouldBlock`]. `ENOBUFS` has no `ErrorKind` of its own (it lands in
/// `Uncategorized`, which is unstable to match on), so it is compared by raw errno.
fn is_retryable(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::Interrupted {
        return true;
    }
    e.raw_os_error() == Some(libc::ENOBUFS)
}

/// Write `text` to `out` with pi's retry loop (`writeRawStdout` → `writeRawStdoutChunk`,
/// `output-guard.ts:85`, `:20-43`). An empty `text` is a no-op, matching pi's
/// `if (text.length === 0) return;` (`:86-88`).
///
/// Partial writes are tracked by offset rather than delegating to `write_all`: a `WouldBlock` may
/// arrive *after* some bytes have already been accepted, and re-issuing the whole slice would
/// duplicate them on the wire — a corruption `write_all`'s own retry-on-`Interrupted` contract does
/// not protect against, because it gives the caller no way to learn how far it got.
pub async fn write_raw_stdout<W: Write>(out: &mut W, text: &str) -> io::Result<()> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Ok(());
    }
    let mut written = 0usize;
    while written < bytes.len() {
        match out.write(&bytes[written..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "raw stdout accepted no bytes",
                ));
            }
            Ok(n) => written += n,
            Err(e) if is_retryable(&e) => {
                tokio::time::sleep(Duration::from_millis(RAW_STDOUT_RETRY_DELAY_MS)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Flush `out` with the same retry loop — pi's `flushRawStdout` (`output-guard.ts:105-108`), whose
/// body is `await waitForRawStdoutBackpressure(); await writeRawStdoutChunk("");`. The drain half is
/// mechanism-N/A here (see the module doc); the retrying terminal write is this.
pub async fn flush_raw_stdout<W: Write>(out: &mut W) -> io::Result<()> {
    loop {
        match out.flush() {
            Ok(()) => return Ok(()),
            Err(e) if is_retryable(&e) => {
                tokio::time::sleep(Duration::from_millis(RAW_STDOUT_RETRY_DELAY_MS)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A sink that answers `WouldBlock` for the first `blocks` write attempts, then accepts at most
    /// `chunk` bytes per call — the shape a non-blocking pipe presents.
    struct Flaky {
        blocks: usize,
        chunk: usize,
        buf: Vec<u8>,
        flush_blocks: usize,
    }

    impl Write for Flaky {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            if self.blocks > 0 {
                self.blocks -= 1;
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
            let n = self.chunk.min(data.len());
            self.buf.extend_from_slice(&data[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            if self.flush_blocks > 0 {
                self.flush_blocks -= 1;
                return Err(io::Error::from(io::ErrorKind::WouldBlock));
            }
            Ok(())
        }
    }

    /// TOOL-037 — pi retries `EAGAIN`/`EWOULDBLOCK`/`ENOBUFS` rather than failing
    /// (`output-guard.ts:36-41`), and a partially-accepted write resumes at the offset instead of
    /// re-sending the whole line. Before this helper the modes called `write_all` + `flush`
    /// directly, so a non-blocking stdout — the shape a supervisor or CI pipe presents — turned a
    /// transient condition into a mode-fatal `io::Error`.
    #[tokio::test]
    async fn retries_would_block_and_resumes_at_the_offset() {
        let mut sink = Flaky { blocks: 3, chunk: 4, buf: Vec::new(), flush_blocks: 2 };
        write_raw_stdout(&mut sink, "abcdefghij\n").await.unwrap();
        flush_raw_stdout(&mut sink).await.unwrap();
        assert_eq!(String::from_utf8(sink.buf).unwrap(), "abcdefghij\n");
        assert_eq!(sink.blocks, 0, "every simulated block was retried, not skipped");
        assert_eq!(sink.flush_blocks, 0);
    }

    /// Pi's `if (text.length === 0) return;` (`output-guard.ts:86-88`) — an empty chunk never
    /// reaches the sink at all.
    #[tokio::test]
    async fn an_empty_chunk_is_a_no_op() {
        let mut sink = Flaky { blocks: 99, chunk: 1, buf: Vec::new(), flush_blocks: 0 };
        write_raw_stdout(&mut sink, "").await.unwrap();
        assert_eq!(sink.blocks, 99, "an empty write must not touch the sink");
    }

    /// Anything that is NOT one of pi's three codes rethrows (`output-guard.ts:39-40`) — a broken
    /// pipe must terminate the mode, not spin forever.
    #[tokio::test]
    async fn a_non_retryable_error_propagates() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let err = write_raw_stdout(&mut Broken, "x").await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    }
}
