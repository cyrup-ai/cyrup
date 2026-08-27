---
stage: new
status: done
updated: 2026-08-22 20:30
---

# Pin the RPC Reader I/O Error Path with a Failing-Reader Test

## Description

`SURFACE_RPC_READER_IO_ERROR` made `read_lines` in `crates/cyrup-modes/src/rpc/mod.rs` return
`io::Result<()>`, so a read failure propagates out of `run_rpc` as `ModesError::Io` instead of being
indistinguishable from clean EOF. It also reordered `run_rpc` so the driver's outcome is held until
the write pump has drained — a reader error no longer discards responses already queued behind it.

**None of that is exercised.** Every `run_rpc` test drives a `std::io::Cursor`, which cannot fail;
a grep over `crates/cyrup-modes/src/tests` finds no `io::Error`, `ErrorKind` or `BrokenPipe`
anywhere. So the `Err(e) => return Err(ModesError::Io(e))` arm, the `reader_ended` gating that lets
the join resolve without deadlocking, and the reordered flush are all currently unpinned — a
regression in any of the three would go unnoticed.

The same gap now applies to the client side: `RPC_CLIENT_READER_ERROR_HANDLING` made the stdout pump
latch `RpcClientError::Io` on a read/decode failure, and the stderr pump survive a non-UTF-8 byte.
Neither is covered.

## Fix

Add a reader fixture that yields one good line and then an error — an `AsyncBufRead` impl whose
second `poll_fill_buf` returns `Err(ErrorKind::BrokenPipe)` is enough, and it belongs in
`crates/cyrup-modes/src/tests/support.rs` beside the other shared fixtures.

Host side (`tests/modes/rpc_commands.rs` or a new sibling):
- `run_rpc` over that reader returns `Err(ModesError::Io(..))`, not `Ok(())`.
- The response to the first, good line still reached the writer — this is the assertion that pins
  the reordered flush, and it fails if `driven?` moves back ahead of the pump await.

Client side (`tests/rpc_client.rs`):
- A scripted host whose stdout yields a line then a decode error leaves the client latching
  `RpcClientError::Io`, and a subsequent `send` reports that rather than
  `ProcessExited { code: "null", signal: "null" }`.
- The stderr pump keeps accumulating after a non-UTF-8 byte, and the buffer keeps one line per
  entry with no duplicated newlines.

## Acceptance Criteria

- [ ] A failing-reader fixture lives in `crates/cyrup-modes/src/tests/support.rs` and is used by both the host-side and client-side cases
- [ ] A test asserts `run_rpc` returns `Err(ModesError::Io(..))` when the reader fails mid-stream
- [ ] A test asserts the response to the pre-error line still reached the writer, and fails if the `driven?` / write-pump ordering is reverted
- [ ] A test asserts the client latches `RpcClientError::Io` rather than `ProcessExited` on a stdout read/decode failure
- [ ] A test asserts the stderr pump keeps accumulating past a non-UTF-8 byte, one line per entry
- [ ] `cargo test -p cyrup-modes` passes; `cargo clippy -p cyrup-modes --all-targets --no-deps` is clean

## Source

- Flagged by the wave-2 gate of the `remediate-cyrup-modes-hygiene` workflow while verifying `SURFACE_RPC_READER_IO_ERROR`
- Severity: medium | Size: small
