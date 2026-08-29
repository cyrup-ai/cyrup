---
stage: new
status: pending
updated: 2026-08-29 02:33
---

# Get the per-entry `fdatasync` off the session-write path

> **Measured: 1022 µs per persisted entry vs 3.3 µs without the sync — 310×.** On ext4
> (`/dev/sda1`, `commit=30`). Every persisted session entry pays a full device flush,
> synchronously, on the caller's thread.

---

## 0. READ THIS FIRST — two traps, one of which will make you dismiss the task

**(a) Measuring this on `/tmp` reports NO DIFFERENCE, and `/tmp` here is tmpfs.**

```
  on /tmp (tmpfs, RAM-backed):
    open+write+close            :      3.1 us/entry
    open+write+fdatasync+close  :      3.2 us/entry      <- fsync is a no-op on tmpfs
```
```
  on $HOME (ext4, /dev/sda1):
    open+write+close            :      3.3 us/entry
    open+write+fdatasync+close  :   1022.3 us/entry      -> 310x
```

The first measurement was taken by accident during the audit that produced this task and it
says the cost is zero. It is wrong. **Measure on the real filesystem the session file
actually lives on.** `df -h /tmp` here shows a separate 16 GB tmpfs; see the workspace notes
on that trap.

**(b) This is NOT a straight parity gap — cyrup is doing something pi does not, on
purpose.** pi persists with `appendFileSync`
([`session-manager.ts:1021,1040`](../../../../pi/packages/coding-agent/src/core/session-manager.ts)),
which is `open(O_APPEND)` + `write` + `close` and **never** calls `fsync`/`fdatasync`.
cyrup's [`store.rs:57`](../../../crates/cyrup-session/src/store.rs) adds `f.sync_data()?`.

So cyrup offers a **strictly stronger durability guarantee** than pi — a session survives a
machine power-loss, not merely a process crash — and pays 310× per entry for it. That is a
real trade someone may have made deliberately. **This task's first job is to establish
whether it was a decision or an accident**, not to delete the line.

---

## 1. Where it is

[`crates/cyrup-session/src/store.rs:51-59`](../../../crates/cyrup-session/src/store.rs):

```rust
    fn append_line(&mut self, line: &str) -> Result<(), SessionError> {
        // One `write` of `<json>\n` to an append-mode fd: a crash mid-write leaves a partial
        // final line that the tolerant reader drops ("last good line wins" — R-04-032).
        let mut f = OpenOptions::new().create(true).append(true).open(&self.path)?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()?;
        f.sync_data()?;
        Ok(())
    }
```

Four costs, in descending order:

1. `sync_data()` — **~1019 µs of the 1022**. The whole problem.
2. `OpenOptions::…open()` **per call** — the fd is not held. An `open`+`close` pair per
   entry, where one long-lived append fd would do.
3. `f.flush()` — a no-op on a bare `File` (no `BufWriter`), harmless.
4. It is **synchronous on the caller's thread**. The caller is
   [`manager/mod.rs:147-158`](../../../crates/cyrup-session/src/manager/mod.rs)
   (`persist_last`), reached from the message-append path — so the ~1 ms lands inside the
   agent's turn, once per persisted entry.

`rewrite` ([`store.rs:62-93`](../../../crates/cyrup-session/src/store.rs)) also syncs, but
it runs only on a migration or an eager clone seed — twice in a session's life, not per
entry. **Leave `rewrite` alone**; its sync is cheap in aggregate and it is guarding the
one moment the user's only copy of their history is rebuilt from memory.

---

## 2. The decision this task must make first

Three options. **Pick one and write the reasoning into the doc comment**, because the next
reader will otherwise re-litigate it.

| option | durability | cost/entry | matches pi? |
| --- | --- | --- | --- |
| **A. Drop `sync_data()`** | survives process crash, not power loss | ~3 µs | yes, exactly |
| **B. Keep the sync, move it off the caller** | unchanged | ~0 µs observed; 1 ms on a writer thread | no (stronger) |
| **C. Batch the sync** — write per entry, `sync_data` on a debounce or at turn end | bounded window (one turn) | amortised | no (stronger) |

**Recommendation: B, then C if the writer thread ever becomes a bottleneck.** Reasons:

- Option A is a *regression in a guarantee cyrup currently provides*. Removing durability
  to gain speed, when the speed is available without removing it, is the wrong trade.
- Option B is pure win: the same fsync happens, just not on the thread the user is waiting
  on. Session writes are already append-only and ordered, so a single-consumer writer task
  preserves ordering trivially.
- Option C is the fallback if fsync throughput (not latency) turns out to matter — e.g. a
  chatty tool-result stream at >1000 entries/s, which is not a shape this workload has.

**If someone knows this was a deliberate durability decision, B honours it. If it was an
accident, B costs nothing to keep. Either way B is safe** — which is why it is the
recommendation despite the question in §0(b) being open.

---

## 3. Required implementation (option B)

1. **Hold the fd.** `DiskStore` gains an `Option<File>` opened once in append mode and
   reused, replacing the per-call `OpenOptions::…open()`. Reopen on an error rather than
   caching a broken fd. The "one `write` of `<json>\n`" atomicity note at
   [`store.rs:52-53`](../../../crates/cyrup-session/src/store.rs) still holds — `O_APPEND`
   is what provides it, not the fresh open.

2. **Move the write to a dedicated writer.** A `std::thread` (not a tokio task — this is
   blocking file I/O and it must not occupy a runtime worker) fed by an unbounded channel
   of `(path, line)`:
   - Single consumer ⇒ append order is preserved without a lock.
   - `append_line` becomes a channel send: sub-microsecond, never blocks the turn.
   - The channel must be **unbounded**, or a full channel reintroduces the stall it exists
     to remove.

3. **Do not lose data at shutdown.** This is the part that makes the change non-trivial and
   it must be explicit:
   - The writer drains its queue and syncs before the process exits. Wire it into the same
     shutdown path that already handles terminal restore, not into `Drop` on an arbitrary
     thread.
   - A write error must surface. Today `append_line` returns `Result` to a caller that can
     react; after the change the caller has already moved on. Route errors to the session's
     existing error/diagnostic channel — **silently dropping a persistence failure is
     strictly worse than the 1 ms it saves.**

4. **`create_exclusive` and `rewrite` stay synchronous.** They are the first-flush and
   migration paths ([`manager/mod.rs:156-160`](../../../crates/cyrup-session/src/manager/mod.rs)),
   they run once, and both need their result before the session can proceed.

5. **Record the durability decision** in `store.rs`'s doc comment: that cyrup fsyncs where
   pi's `appendFileSync` does not, that this is deliberate, and that the cost is kept off
   the turn by the writer thread. Cite
   [`session-manager.ts:1021,1040`](../../../../pi/packages/coding-agent/src/core/session-manager.ts).

---

## 4. Definition of Done

1. **A persisted entry costs the caller <10 µs**, measured on the real filesystem (ext4,
   not tmpfs — see §0a), down from ~1022 µs.
2. **Durability is unchanged.** Data still reaches the device: after the writer has
   drained, a `SIGKILL` of the process loses nothing that `append_line` returned success
   for.
3. **Ordering is exact.** Entries appear in the file in the order `append_line` was called,
   including across a rapid burst and across two sessions writing to different files.
4. **Nothing is lost at shutdown.** A normal exit, a `/quit`, and a `SIGINT` each leave a
   complete session file. The last entry written before exit is present.
5. **A write failure is visible.** A full disk or a permissions error surfaces to the user
   rather than being swallowed by the writer thread. (Note the workspace has hit ENOSPC
   mid-run before; this path must not go quiet when it happens again.)
6. **The tolerant-reader invariant survives.** A crash mid-write still leaves at most one
   partial final line, which the reader drops — R-04-032, `store.rs:52-53`.
7. **No tokio worker is occupied by file I/O.** The writer is an OS thread; `append_line`
   does not block a runtime worker, and no `spawn_blocking` is used per entry.
8. **The suite is green under the real gate:**
   `cargo test --workspace --features test-fixtures --no-fail-fast`, and
   `cargo clippy --workspace --all-targets --features test-fixtures` exits **0**.

### The probe

```python
import os, time, tempfile
d = tempfile.mkdtemp(dir=os.path.expanduser("~"))   # MUST be the real fs, not /tmp
p = os.path.join(d, "s.jsonl")
line = b'{"x":"' + b'y'*400 + b'"}\n'
def run(fsync, n=1000):
    open(p, "wb").close()
    t = time.perf_counter()
    for _ in range(n):
        fd = os.open(p, os.O_WRONLY | os.O_APPEND | os.O_CREAT)
        os.write(fd, line)
        if fsync: os.fdatasync(fd)
        os.close(fd)
    return (time.perf_counter() - t) / n * 1e6
a, b = run(False), run(True)
print(f"no fsync {a:.1f} us | fdatasync {b:.1f} us -> {b/a:.0f}x")
```
