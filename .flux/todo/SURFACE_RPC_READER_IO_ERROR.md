---
stage: new
status: done
updated: 2026-08-22 18:45
---

# Surface the RPC Reader's I/O Error Instead of Treating It as EOF

## Problem

`read_lines` in `crates/cyrup-modes/src/rpc.rs:1656` ends its loop with:

```rust
Err(_) => break,   // rpc.rs:1674
```

A genuine read failure on the RPC input transport (broken pipe, `EIO` on a serial/socket fd, a supervisor tearing the fd down) is therefore handled identically to `Ok(0) => break, // EOF` at `rpc.rs:1661`. The error value is discarded and nothing else observes it: the task is spawned detached at `rpc.rs:781` and its `JoinHandle` is only `abort()`ed at `rpc.rs:975`, never joined — so even a panic in it is invisible. The signature returns `()` (`rpc.rs:1656`), so there is no error channel at all, and the doc comment at `rpc.rs:1646-1647` only says "Ends at EOF or when the receiver drops".

Consequently `rpc_driver` contains **zero** `?` operators and returns `Ok(())` unconditionally at `rpc.rs:976`; its `Result<(), ModesError>` return type is vestigial, and the only error `run_rpc` can ever produce comes from `write_pump`. The consequence reaches the binary: `run_rpc_dispatch` does `let ran = run_rpc(...).await;` then `ran?;` (`crates/cyrup/src/run.rs:115` and `:118`), so `cyrup --mode rpc` exits 0 with no diagnostic after its command stream died mid-protocol — indistinguishable from a client that closed stdin cleanly. An embedder awaiting `run_rpc` gets `Ok(())` and has no way to learn the session was cut off.

## Fix

Give `read_lines` a channel back for its failure.

1. Change the signature to `async fn read_lines<R: AsyncBufRead + Unpin>(mut reader: R, tx: mpsc::Sender<String>) -> io::Result<()>`, returning `Ok(())` on the `Ok(0)` EOF arm (`rpc.rs:1661`) and on the send-failed break (`rpc.rs:1670-1672`), and `Err(e)` at the current `Err(_) => break` arm (`rpc.rs:1674`).
2. In `rpc_driver`, keep the `JoinHandle` from the spawn at `rpc.rs:781`. At the existing clean-shutdown break — the `!reader_open && !in_flight && dispatches.is_empty()` block at `rpc.rs:956-971` — join the handle **after** the existing drain of buffered events/extension errors, and return `Err(ModesError::Io(e))` if it produced one, so everything already queued still reaches `write_pump`.
3. **Do not join at every break.** `rpc.rs:953` sets `reader_open = false;` on the `ctx.shutdown()` path while the reader task is still blocked on stdin; joining there would deadlock. Use a shared slot / oneshot or a `try_join`-style non-blocking read of the result on that path instead, or only join on the EOF path where the task has already finished.
4. Update the doc comment at `rpc.rs:1646-1647` to state all three terminations: EOF, receiver dropped, read error.

The clean-EOF path and every existing emission must stay byte-for-byte identical; only the previously silent failure path changes from `Ok(())` to `Err`.

## Acceptance Criteria

- [ ] `read_lines` in crates/cyrup-modes/src/rpc.rs returns a `Result`, and the former `Err(_) => break` arm propagates the `io::Error` rather than discarding it
- [ ] `rpc_driver` returns `Err(ModesError::…)` when the reader task ended on a read error, and still returns `Ok(())` on clean EOF and on the `ctx.shutdown()` path
- [ ] The error is returned only after the existing drain of buffered events / extension errors, so no already-queued output is lost
- [ ] No deadlock on the `ctx.shutdown()` path (rpc.rs:953) where the reader task is still blocked on input — verified by `cargo test -p cyrup-modes` completing without hanging
- [ ] The doc comment on `read_lines` names all three terminations (EOF, receiver dropped, read error)
- [ ] `cargo test -p cyrup-modes` passes with the same result set as before
- [ ] `cargo clippy -p cyrup-modes --all-targets --no-deps` reports no new warnings

## Source

- Identified by the cyrup-modes hygiene audit (workflow `cyrup-modes-hygiene-audit`)
- Severity: medium | Size: small
