---
stage: new
status: done
updated: 2026-08-22 18:45
---

# Stop Collapsing Read Errors into EOF in the RpcClient stdout and stderr Pumps

## Problem

Both background pumps in `crates/cyrup-modes/src/rpc_client.rs` use `while let Ok(Some(line)) = lines.next_line().await`, which matches `Err(_)` and `Ok(None)` with the same pattern and so cannot distinguish a read/decode failure from clean EOF. `tokio::io::Lines::next_line` returns `Err(InvalidData)` on a byte sequence that is not valid UTF-8, and `Err` on any read failure.

**1. stdout pump misreports a decode error as "Agent process exited".** `rpc_client.rs:1267` is the loop; on any `Err` it falls out and `rpc_client.rs:1270-1275` latches `RpcClientError::ProcessExited { code: "null", signal: "null", stderr }` and calls `reject_pending_requests()`. Every subsequent `send` then fails with "Agent process exited (code=null signal=null)" via the latched-error pre-empt at `rpc_client.rs:1091-1093`, even though the child is alive and healthy. The embedder is handed a factually wrong diagnosis and the real cause is dropped on the floor. This is also asymmetric with the crate's own host-side reader, which deliberately decodes lossily (`String::from_utf8_lossy`, `crates/cyrup-modes/src/rpc.rs:1669`) precisely so a stray non-UTF-8 byte does not kill the transport. `RpcClient::attach` is public, so the peer is not guaranteed to be a cyrup host. The doc block at `rpc_client.rs:1246-1260` covers EOF only; the `[CYRUP-DELTA]` note at `:1252` does not cover the error path.

**2. stderr pump silently stops collecting.** The pump spawned in `RpcClient::spawn` uses the same pattern at `rpc_client.rs:570` (body `:568-579`). The first `Err` permanently terminates the task, so from that point nothing is accumulated into `inner.stderr` and nothing is passed through to this process's stderr. That buffer is the **only** diagnostic payload carried by `ProcessExited`, `ProcessError`, `StdinNotWritable`, `RequestTimeout`, `IdleTimeout` and `CollectTimeout` (`rpc_client.rs:141, 148-149, 152-153, 156-157, 160-161, 164-165`), read through `stderr_snapshot()` at `rpc_client.rs:269-274` — so the failure this truncates is exactly the one whose explanation the embedder needs. The comment at `rpc_client.rs:566` states the intent is to mirror pi's `on("data")` accumulate-and-passthrough, which operates on raw bytes and has no decode failure mode at all, so this is also a faithfulness delta.

## Fix

**stdout pump (`rpc_client.rs:1264-1276`)** — match the three cases explicitly:

```rust
loop {
    match lines.next_line().await {
        Ok(Some(line)) => inner.handle_line(&line),
        Ok(None) => break,
        Err(e) => { inner.set_exit_error(RpcClientError::Io(e)); break }
    }
}
```

Keep the existing `set_exit_error(ProcessExited{..})` / `reject_pending_requests()` tail for the EOF path — the first-error-wins latch at `rpc_client.rs:275-283` makes the `Io` error take precedence when it was set first. The `RpcClientError::Io(#[from] std::io::Error)` variant already exists at `rpc_client.rs:180`. Extend the doc block at `rpc_client.rs:1246-1260` to name the error path.

**stderr pump (`rpc_client.rs:568-579`)** — read bytes and decode lossily rather than line-strict, matching `rpc.rs:1669`:

```rust
let mut buf = Vec::new();
loop {
    buf.clear();
    match reader.read_until(b'\n', &mut buf).await {
        Ok(0) => break,
        Ok(_) => { let line = String::from_utf8_lossy(&buf); /* existing accumulate + eprintln */ }
        Err(_) => break,
    }
}
```

so a malformed byte degrades one line instead of ending collection. Keeping the loop alive is the whole fix here; no error needs to be surfaced to the caller. Preserve the trailing-newline handling so the accumulated buffer keeps its current one-line-per-entry shape and the passthrough `eprintln!` does not double the newline.

## Acceptance Criteria

- [ ] The stdout pump in crates/cyrup-modes/src/rpc_client.rs distinguishes `Ok(Some)`, `Ok(None)` and `Err`, and a read/decode error latches `RpcClientError::Io` rather than `ProcessExited { code: "null", signal: "null" }`
- [ ] Clean EOF still latches `ProcessExited { code: "null", signal: "null", stderr }` and still calls `reject_pending_requests()` — unchanged behaviour on that path
- [ ] The stderr pump continues accumulating and passing through subsequent lines after a non-UTF-8 byte, and the accumulated buffer keeps one line per entry with no duplicated newlines
- [ ] The doc block above the stdout pump documents the error termination alongside the existing EOF `[CYRUP-DELTA]` note
- [ ] `cargo test -p cyrup-modes` passes with the same result set as before
- [ ] `cargo clippy -p cyrup-modes --all-targets --no-deps` reports no new warnings

## Source

- Identified by the cyrup-modes hygiene audit (workflow `cyrup-modes-hygiene-audit`)
- Severity: medium | Size: small
