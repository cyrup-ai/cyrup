//! Strict-LF JSONL framing for the RPC protocol stream (Pi `modes/rpc/jsonl.ts`): the single-line
//! writer every outgoing record goes through, and the reader that splits the incoming command
//! stream.

use std::io;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::error::ModesError;

use super::RpcOut;

/// Serialize one protocol record and write it as a single LF-terminated line, flushed immediately so
/// the peer never waits on buffering (R-11-013).
pub(super) async fn write_out<W: AsyncWrite + Unpin>(writer: &mut W, out: &RpcOut) -> Result<(), ModesError> {
    let mut line = serde_json::to_string(out)?;
    line.push('\n');
    // TOOL-037 note — pi's RPC writer is `writeRawStdout` (`rpc-mode.ts:60`), whose retry loop
    // (`core/output-guard.ts:36-41`) covers `EAGAIN`/`EWOULDBLOCK`/`ENOBUFS`. This sink is a tokio
    // `AsyncWrite`, not a `std::io::Write`: the executor's own readiness machinery IS that loop —
    // a would-block registers a waker and re-polls instead of surfacing the error — so
    // `crate::raw_stdout`'s explicit sleep-and-retry is the SYNC-sink half only and would be a
    // second, worse implementation here. `ENOBUFS` likewise reaches the caller as a genuine error
    // on both sides.
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

/// Read strict-LF JSONL lines from `reader` and forward **every** record over `tx`. Splits on
/// `\n` only; a trailing `\r` is stripped (CRLF tolerance). Ends on any of three terminations: EOF
/// on the reader (`Ok(())`), the receiver dropping (`Ok(())`), or a read failure on the input
/// transport (`Err`) — the last is returned rather than swallowed so [`rpc_driver`](super::rpc_driver) can tell a
/// severed command stream apart from a client that closed its end cleanly.
///
/// SEAM-054 — an EMPTY line is forwarded like any other, because pi forwards it: `emitLine` is
/// invoked for every newline-delimited slice with no emptiness filter (`modes/rpc/jsonl.ts:25-41`
/// @v0.84.1, the emit at `:38`), it reaches `handleInputLine` (`rpc-mode.ts:748-762`), `JSON.parse("")`
/// throws, and pi answers `error(undefined, "parse", "Failed to parse command: …")` on stdout
/// (`:752-758`). Dropping it here made cyrup silent for an input class pi replies to, so any client
/// correlating n-lines-in to n-responses-out desynchronised. [`dispatch`](super::dispatch)'s existing
/// `serde_json::from_str` failure arm produces pi's exact response with no id.
pub(super) async fn read_lines<R: AsyncBufRead + Unpin>(
    mut reader: R,
    tx: mpsc::Sender<String>,
) -> io::Result<()> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => return Ok(()), // EOF
            Ok(_) => {
                if buf.last() == Some(&b'\n') {
                    buf.pop();
                }
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                let line = String::from_utf8_lossy(&buf).into_owned();
                if tx.send(line).await.is_err() {
                    // The driver stopped listening; nothing left to deliver to.
                    return Ok(());
                }
            }
            Err(e) => return Err(e),
        }
    }
}
