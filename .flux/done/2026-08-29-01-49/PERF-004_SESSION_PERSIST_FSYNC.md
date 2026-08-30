---
stage: qa
status: completed
updated: 2026-08-30 00:35
aug_against: cyrup HEAD cd16fe9 (main, after PERF-003 landed as PR #107) · rust-version 1.96 · edition 2024 · zero new dependencies
measured_on: this host, ext4 on /dev/vda. Probe + reference implementation rebuilt and re-verified
  this pass at [`tmp/perf004-verify/`](../../../tmp/perf004-verify/) (gitignored — see §0d).
---

# Get the per-entry `fdatasync` off the session-write path

> **Measured on this host: ~209× per persisted entry (≈214 µs → 1.02 µs), head-to-head in real
> Rust in one process (§7.2).** Every persisted session entry pays a full device flush,
> synchronously, on a tokio worker thread, while holding the session mutex.
>
> Through the complete prescribed `append_line` — sticky-error slot, `Arc` clones, channel send and
> all — the caller pays **1.50 µs/entry** (§8). Every durability guarantee cyrup has today is kept.

---

## 0. READ THIS FIRST

### 0a. What this AUG-5 pass changed, and why you must not skip it

The previous revision was written at HEAD `7913760` **on a different host**. `store.rs` is
byte-identical since then (`git diff 7913760 HEAD -- crates/cyrup-session/src/store.rs` is empty)
and **all 41 `file:line` citations in this file were re-checked one by one and every one still
resolves exactly**. Four things did change, and two of them would have blocked `/exec`:

1. **🔴 THE REFERENCE IMPLEMENTATION THIS FILE TOLD YOU TO TRANSCRIBE FROM NO LONGER EXISTS.**
   §3, §6 and §8 all pointed at `tmp/perf004-verify/store-option-c-verified.rs` with the
   instruction *"Transcribe from there; do not re-derive."* `tmp/` is gitignored
   (`.gitignore:7`), so it never survived the container. **§3 below is now the authoritative,
   self-contained source** — it was rebuilt from these listings this pass, compiled, clippy'd and
   run (§8). Nothing outside this file is load-bearing any more.
2. **🔴 TWO OF THE THREE §3.5 CALL SITES COULD NOT COMPILE AS WRITTEN.** Neither `cyrup` nor
   `cyrup-tui` has a direct `cyrup-session` dependency, so neither can name
   `cyrup_session::flush_session_writes`. §3.5 now carries the house-style fix (a re-export from
   `cyrup-session-svc`) and it needs no new dependency edge.
3. **🟡 THE HEADLINE NUMBER IS HOST-DEPENDENT AND THE OLD ONE IS WRONG HERE.** The previous host
   measured `fdatasync` at ~1106 µs (946×). This host measures ~200-244 µs (**~209×**). The
   *conclusion* is unaffected — it is still two-to-three orders of magnitude — but do not quote 946×.
4. **🟢 THE STICKY ERROR IS UPGRADED FROM `String` TO `std::io::Error`.** The old spelling round-tripped
   through `io::Error::other(msg)`, which **discards `ErrorKind`** — so a deferred ENOSPC arrived as
   `ErrorKind::Other` and became undetectable. DoD 5 is explicitly about ENOSPC. The typed slot
   compiles, clippy's clean, and §8's test now asserts `ErrorKind::StorageFull` survives.

### 0b. Do NOT trust this file's filesystem claims — run `df -T` yourself

The previous revision asserted *"`/tmp` here is tmpfs"*. **That is false on this host.** The
container changed underneath the task:

```
  previous host:  /  = ext4 on /dev/root (96 G)   |  /tmp = separate 16 G tmpfs
  THIS host:      /  = ext4 on /dev/vda  (252 G)  |  /tmp = the SAME ext4 filesystem
```

The rule that survives is the one that was always the point: **measure on a real block device, never
on tmpfs**, because on tmpfs `fdatasync` is nearly free and the whole task reads as a no-op. Confirm
with `df -T "$HOME"` before you believe any number you produce. On *this* host `$HOME` and `/tmp` are
the same ext4 volume, so either works — that will not be true on the next one.

### 0c. The obvious answer is wrong. Read §2 before writing code.

An earlier draft recommended *"keep the sync, move the whole write to a background writer thread"*
(option B). B **weakens durability below pi's** — the exact opposite of what it claims — and breaks
read-after-write for five production call sites. The correct answer is **option C**; it is smaller
than B and needs no queue, no fence, no shutdown protocol and no error-routing plumbing.

### 0d. Two things this file cites are not present in this container

- `tmp/perf004-verify/` was **rebuilt this pass** and re-verified, but it is gitignored and will not
  survive. Everything you need is inline in §3; §8 says how to regenerate the harness if you want it.
- **The `pi` source tree is absent** (`../pi` does not exist). The pi citations below
  (`session-manager.ts:1015-1042`) were verified in an earlier pass and are carried forward on that
  authority. §3.6 asks you to restate them in a doc comment — restate them, do not go hunting for
  `../pi`, and do not weaken the comment because you could not re-open the file.

---

## 1. Where it is

[`crates/cyrup-session/src/store.rs:51-59`](../../../crates/cyrup-session/src/store.rs) (line
numbers exact at HEAD `cd16fe9`):

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

Five defects, in descending order of cost:

1. **`sync_data()` — ~200 µs of the ~214.** The whole problem.
2. **`OpenOptions::…open()` per call.** The fd is not held: an `open`+`close` pair per entry where
   one long-lived `O_APPEND` fd would do. Measured cost of the reopen: **3.07 µs vs 1.10 µs held**,
   i.e. the reopen is ~3× the entire cost of the write it wraps.
3. **The comment at `:52-53` is false of the code at `:54-55`.** It claims "One `write` of
   `<json>\n`" — the code issues **two** `write(2)` calls, payload then newline. `O_APPEND`
   atomicity is *per `write` call*, so the invariant the comment asserts (and that R-04-032's
   tolerant reader is specified against) does not hold: a concurrent appender on the same file can
   interleave between the two, producing `{"a":1}{"b":2}\n\n`. Coalescing into one buffer is free —
   it is also **~1 µs/entry faster** (2.08 → 1.10) — and makes the comment true.
4. **`f.flush()` is a no-op** on a bare `File` (no `BufWriter`). Harmless, but it reads as if it does
   something.
5. **It is synchronous inside an async lock, on a tokio worker.** The caller chain, re-verified
   end-to-end at HEAD:

   [`subscriber.rs:171`](../../../crates/cyrup-session-svc/src/subscriber.rs) —
   `let _ = self.manager.lock().await.append_message(core);` — inside
   `impl EventSubscriber for SvcSubscriber::on_event`, an `async fn` on a
   `#[tokio::main(flavor = "multi_thread")]` runtime
   ([`main.rs:48`](../../../crates/cyrup/src/main.rs))
   → [`manager/append.rs:24`](../../../crates/cyrup-session/src/manager/append.rs) `append_agent_message`
   → [`manager/mod.rs:127`](../../../crates/cyrup-session/src/manager/mod.rs) `push_entry`
   → [`manager/mod.rs:143`](../../../crates/cyrup-session/src/manager/mod.rs) `persist_last()?`
   → [`manager/mod.rs:147-161`](../../../crates/cyrup-session/src/manager/mod.rs) `persist_last`
   → `store.append_line` (`:154`) — the **only** production caller in the workspace (the
   `append_line` hits in `cyrup-mcp/src/renderers.rs` are an unrelated method on another type).

   So each entry blocks **a runtime worker for ~214 µs** *and* holds
   `Arc<AsyncMutex<SessionManager>>` ([`subscriber.rs:102`](../../../crates/cyrup-session-svc/src/subscriber.rs))
   for that whole time — serializing every other manager access (`session_file()`, fork resolution,
   `list_sessions`, the TUI's tree reads) behind a device flush. A tool-using turn persists `3 + N`
   entries (user, assistant-with-calls, one per tool result, final assistant), so a 5-call turn
   stalls ≈ **1.7 ms** inside the lock, and did ≈ 8.9 ms on the host the previous revision measured.

**Note what is already lost:** `subscriber.rs:171` and `:177-184` both discard the `Result`
(`let _ = …`), and `:171` is the **only** `append_message` call site in `cyrup-session-svc` or
`cyrup-tui`. The highest-volume caller of `append_line` **already** swallows every persistence error
today. That is relevant to DoD 5 — it is a pre-existing hole, and option C is the only option here
that does not widen it. It is also why §3.3's write-then-report ordering is safe.

`rewrite` ([`store.rs:62-93`](../../../crates/cyrup-session/src/store.rs)) also syncs, but it runs
only on a migration or an eager clone/branch seed — twice in a session's life, not per entry.
**Leave `rewrite`'s sync alone.** (It does have one thing that must change: see §3.4.)

---

## 2. The decision, and why the obvious answer is wrong

The durability question is not "does it fsync" — it is **where the bytes are when `append_line`
returns**, because that is what decides which failures lose data.

| | on `append_line` return, the bytes are… | survives `SIGKILL`/`process::exit` | survives power loss | caller cost/entry |
| --- | --- | --- | --- | --- |
| **pi** (`appendFileSync`) | in the **page cache** | ✅ yes | ❌ no | ~3 µs |
| **cyrup today** | on the **device** | ✅ yes | ✅ yes | **~214 µs** |
| **A.** drop `sync_data()` | in the page cache | ✅ yes | ❌ no | ~1.1 µs |
| **B.** async writer thread | in a **userspace queue** | ❌ **NO** | ❌ no (until drained) | ~0.5 µs |
| **C.** sync write, async sync | in the **page cache** | ✅ yes | ✅ yes, after ≤1 round | **1.50 µs (measured, §8)** |

Read the `SIGKILL` column. **Option B is the only row that loses data to a process crash** — the
queue dies with the process. B is not "the same durability, moved off the caller"; it is *weaker
than pi*, in exchange for ~1 µs over option C. Everything B then has to build — the drain-at-shutdown
protocol, the error re-routing, the unbounded channel — exists solely to partially claw back a
guarantee it just gave away.

The page cache is the key fact: **`write(2)` hands ownership of the bytes to the kernel.** The
process can be `SIGKILL`ed, `abort()`ed, or `std::process::exit`ed and the data still reaches the
disk, because the kernel is what writes it back. `fdatasync` only adds *power-loss* durability. So
splitting the two — keep the `write` synchronous, move only the `fdatasync` — keeps every guarantee
cyrup has today and pays ~1.1 µs for it.

**C also costs nothing to prove correct**, which B cannot claim:

- **Read-after-write is preserved for free.** The page cache is coherent: any reader — including a
  *different process* — sees the bytes the instant `write` returns. Under B, **five** production
  sites read a live session's file and would see a stale tail (all five re-verified at HEAD):
  - [`fork_context.rs:491`](../../../crates/cyrup-ext-subagents/src/fork_context.rs) —
    `SessionManager::open(&persisted_path)?` re-reads the **live parent's file from disk** to build
    a subagent's forked branch (the code documents it as a "THROWAWAY handle on the parent's
    PERSISTED file", `:487`). A queued tail means the subagent silently starts from a truncated
    transcript.
  - [`session/files.rs:79`](../../../crates/cyrup-session-svc/src/session/files.rs) —
    `SessionManager::open(path)?` then `append_session_info` on **another** session's file
    (`rename_session_file`, `:69`).
  - [`session/files.rs:160`](../../../crates/cyrup-session-svc/src/session/files.rs) —
    `rename_session_file_at`, the same two calls made by the **pre-launch** `--resume` picker
    before any session service exists (SEAM-062).
  - [`runtime.rs:606`](../../../crates/cyrup-session-svc/src/runtime.rs) — `SessionManager::open(&file)?`
    on a session switch/branch.
  - [`listing.rs:55`](../../../crates/cyrup-session/src/listing.rs) — `list_in_dir` streams every
    file for the `/resume` picker's titles and message counts.
- **Ordering is exact by construction.** Writes stay on the caller's thread behind `&mut self`;
  there is no second writer to order against.
- **No test changes.** At least a dozen tests append and then immediately read the file — e.g.
  [`tests/sessions.rs:71,116`](../../../crates/cyrup-session/src/tests/sessions.rs)
  (`std::fs::read_to_string(&file)` immediately after the append that creates it), and
  [`tests/parity.rs:585-593`](../../../crates/cyrup-session/src/tests/parity.rs), which drives a
  bare `DiskStore` directly. Under B each needs a fence; under C each is untouched.
- **The write error stays synchronous.** ENOSPC/EACCES/EDQUOT still return from `append_line`
  exactly as today. Only the *sync* error is deferred, and §3.3 routes it back into the same
  `Result` — **with its `ErrorKind` intact** (§0a.4).
- **Shutdown stops being a correctness requirement.** Under B, missing a drain loses user data.
  Under C, missing the final sync degrades power-loss durability for the last few entries and
  nothing else. The barrier in §3.5 is still worth wiring; it is no longer load-bearing.

**Decision: implement option C.** Record the reasoning in the `store.rs` doc comment (§3.6) so the
next reader does not re-litigate it — including the fact that cyrup deliberately fsyncs where pi
does not.

### Why the batched sync needs no timer

Measured on this host, ext4 (§7.1 for the probe):

```
  write only (held fd, one write)  :     1.10 us/entry
  write only (held fd, two writes) :     2.08 us/entry     <- what the code does today
  reopen + write, no sync          :     3.07 us/entry     <- the per-call open+close today
  write + fdatasync every entry    :   188.37 us/entry     <- what the code does today
  write + fdatasync every 8        :    29.48 us/entry
  fdatasync with nothing dirty     :    49.46 us/call
```

A background syncer that simply **drains everything queued behind the message that woke it, dedups,
and syncs once** self-tunes with no configuration: at one entry per turn it syncs once per entry
(off-thread, invisible); during a burst, every entry that arrives inside the ~200 µs flush collapses
into the *next* single flush. Adding a debounce timer would only make the quiet case worse. The
`49.46 µs` row is why the dedup is worth having rather than syncing per queued message — on this
host an empty flush costs **26%** of a real one, up from 3% on the previous host, so the dedup earns
more here than the previous revision assumed.

---

## 3. Required implementation (option C)

All of it is in [`crates/cyrup-session/src/store.rs`](../../../crates/cyrup-session/src/store.rs)
except §3.5. **No dependency is added** — `std::sync::mpsc`, `LazyLock` and `std::thread` are enough,
and all are available at the workspace's `rust-version = "1.96"` (`Cargo.toml:89`; `LazyLock`
stabilised 1.80, `io::Error::other` 1.74). `cyrup-session` has **no logging dependency** — re-verified
this pass: `rg 'tracing|log::|eprintln|println!' crates/cyrup-session/src` matches **zero** lines —
and must not gain one; the error path in §3.3 is built on the existing `Result` for that reason.

> **The listings in §3.0–3.4 ARE the reference implementation.** They were transcribed into a
> standalone crate carrying `#![forbid(unsafe_code)]` and the workspace's exact clippy deny set,
> then compiled (exit 0), clippy'd (**exit 0, zero warnings**) and run under seven behavioural
> tests (**7 passed, 0 failed**) — this pass, on this host. See §8. Transcribe from here.

### 3.0 Imports to add at the top of `store.rs`

```rust
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, LazyLock, Mutex};
```

### 3.1 The process-global syncer

Mirror the precedent and the doc-comment style of `FILE_MUTATION_LOCKS`
([`cyrup-tools/src/lock.rs:28`](../../../crates/cyrup-tools/src/lock.rs)): a `LazyLock` static whose
comment justifies *why* it is global rather than per-owner. That file is the house style for exactly
this decision — read its comment before writing yours. (`LazyLock` is used in **11** files across the
workspace and `std::thread::Builder` in **4** — `cyrup-provider/src/auth/oauth/callback.rs`,
`cyrup-tui/src/{clipboard,open_browser}.rs`, and
[`spawn/signal.rs`](../../../crates/cyrup-ext-subagents/src/spawn/signal.rs), the last of which is
`#[cfg(all(test, unix))]`, so the three production precedents are the first three. Both counts
re-verified this pass.)

Global, not per-`DiskStore`, because store instances are not stable: `adopt_branch` **replaces**
`self.store` wholesale
([`branched_session.rs:145`](../../../crates/cyrup-session/src/manager/branched_session.rs)),
`fork_context` opens a throwaway `SessionManager` on a live file, and `rename_session_file` opens a
second manager on another file. One thread for the process bounds the cost at one OS thread total and
gives a single total order over every session write in the process.

```rust
/// A background `fdatasync` request: the handle to flush, plus the slot its failure is
/// reported through (see `DiskStore::sync_err`).
enum SyncReq {
    Sync { file: Arc<File>, err: Arc<Mutex<Option<std::io::Error>>> },
    /// Answered once every request queued ahead of it has been flushed.
    Barrier(Sender<()>),
}

/// The one session-fsync worker for the whole process.
///
/// `DiskStore::append_line` keeps its `write(2)` synchronous — so the bytes are in the page
/// cache, and therefore visible to every reader and safe against a process crash, before it
/// returns — and hands only the `fdatasync` here. Global rather than per-store because
/// `SessionManager` swaps its `Box<dyn SessionStore>` on a branch
/// (`manager/branched_session.rs:145`) and several call sites open a second manager over a live
/// file, so a per-store worker would neither bound the thread count nor order those writes
/// against each other.
static SESSION_SYNCER: LazyLock<Syncer> = LazyLock::new(Syncer::start);

struct Syncer {
    /// `None` when the worker thread could not be spawned — `request` then syncs inline, i.e.
    /// degrades to exactly today's behaviour rather than silently dropping durability.
    tx: Option<Sender<SyncReq>>,
}
```

> **The error slot is `std::io::Error`, NOT `String`.** An earlier revision stored `e.to_string()`
> and rebuilt it with `io::Error::other(msg)`, which **erases `ErrorKind`** — a deferred ENOSPC
> arrived as `ErrorKind::Other` and no caller could recognise it. DoD 5 is specifically about
> ENOSPC. `std::io::Error` is `Send + Sync`, so it crosses the channel and lives in the `Mutex`
> with no wrapper; §8's test asserts `ErrorKind::StorageFull` survives the round trip.

`Syncer::start` matches on the spawn `Result` — **no `unwrap`/`expect`** (`[workspace.lints.clippy]`
at `Cargo.toml:97-101` denies `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`; they fire
only under `cargo clippy`). On `Err`, return `Syncer { tx: None }`.

```rust
impl Syncer {
    fn start() -> Self {
        let (tx, rx) = channel::<SyncReq>();
        match std::thread::Builder::new()
            .name("cyrup-session-fsync".into())
            .spawn(move || run(rx))
        {
            Ok(_handle) => Self { tx: Some(tx) },
            // Spawn failed (EAGAIN / thread rlimit). Fall back to syncing inline — i.e. exactly
            // today's cost and today's guarantee — rather than dropping the flush silently.
            Err(_) => Self { tx: None },
        }
    }

    fn request(&self, file: &Arc<File>, err: &Arc<Mutex<Option<std::io::Error>>>) {
        let Some(tx) = &self.tx else {
            return Self::sync_inline(file, err);
        };
        let req = SyncReq::Sync { file: Arc::clone(file), err: Arc::clone(err) };
        if tx.send(req).is_err() {
            // The worker can only end if the static's sender is dropped, which never happens for
            // a `LazyLock` static — but if it somehow does, do not lose the flush.
            Self::sync_inline(file, err);
        }
    }

    fn sync_inline(file: &Arc<File>, err: &Arc<Mutex<Option<std::io::Error>>>) {
        if let Err(e) = file.sync_data() {
            let mut slot = err.lock().unwrap_or_else(|p| p.into_inner());
            *slot = Some(e);
        }
    }

    /// Block until every request enqueued before this call has been flushed.
    fn barrier(&self) {
        // In inline mode every sync already completed synchronously; nothing can be pending.
        let Some(tx) = &self.tx else { return };
        let (ack_tx, ack_rx) = channel::<()>();
        if tx.send(SyncReq::Barrier(ack_tx)).is_ok() {
            let _ = ack_rx.recv();
        }
    }
}
```

The worker loop — coalesce, dedup, then ack. **Note the `PendingSync` alias: it is not cosmetic.**
Spelling the vec `Vec<(Arc<File>, Arc<Mutex<Option<std::io::Error>>>)>` inline trips
`clippy::type_complexity`, which is warn-by-default and would break this workspace's zero-warning
steady state (DoD 9). **Re-confirmed this pass by compiling both forms** — without the alias clippy
emits *"very complex type used. Consider factoring parts into `type` definitions"*.

```rust
/// One queued flush: the handle, plus the slot its failure is reported through. Named because
/// `clippy::type_complexity` fires on the bare tuple below and the gate expects zero warnings.
type PendingSync = (Arc<File>, Arc<Mutex<Option<std::io::Error>>>);

fn run(rx: Receiver<SyncReq>) {
    // Reused across rounds to keep the loop allocation-free in steady state.
    let mut round: Vec<PendingSync> = Vec::new();
    let mut acks: Vec<Sender<()>> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();

    while let Ok(first) = rx.recv() {
        round.clear();
        acks.clear();
        seen.clear();

        // Drain everything already queued behind the message that woke us: an entire burst
        // collapses into one flush per file. This is the debounce — no timer needed.
        for req in std::iter::once(first).chain(rx.try_iter()) {
            match req {
                SyncReq::Sync { file, err } => round.push((file, err)),
                SyncReq::Barrier(ack) => acks.push(ack),
            }
        }

        // Dedup by fd identity, not by path: two stores can name one path, and one path can name
        // two inodes across a `rewrite`. Every `Arc` stays alive in `round` for the whole loop,
        // so an address cannot be freed and reused mid-dedup (which would silently skip a real
        // flush). That is why the requests are collected first rather than synced as they drain.
        for (file, err) in &round {
            if !seen.insert(Arc::as_ptr(file) as usize) {
                continue;
            }
            if let Err(e) = file.sync_data() {
                // Sticky, take-once — surfaced by the next `append_line` (§3.3).
                let mut slot = err.lock().unwrap_or_else(|p| p.into_inner());
                *slot = Some(e);
            }
        }

        // Acked only after the whole round flushed, so a barrier strictly follows every request
        // enqueued before it (mpsc is FIFO, so "enqueued before" == "drained before").
        for ack in acks.drain(..) {
            let _ = ack.send(());
        }
        round.clear();
    }
}
```

Poisoned-mutex handling is `unwrap_or_else(|p| p.into_inner())`, matching the house helper at
[`subscriber.rs:26`](../../../crates/cyrup-session-svc/src/subscriber.rs).

`Arc<File>` is what makes this work without a second `open`: `File` is `Send + Sync`,
`File::sync_data(&self)` takes `&self`, and `impl Write for &File` lets the store write through the
same handle. The worker drops its `Arc`s at the end of each round, so a deleted or replaced session
file does not keep an fd on an unlinked inode alive.

### 3.2 Hold the fd

```rust
pub struct DiskStore {
    path: PathBuf,
    /// The held `O_APPEND` handle, opened lazily on the first append and shared with
    /// [`SESSION_SYNCER`]. `None` means "not opened yet, or invalidated" — a write error and
    /// `rewrite`'s inode swap both clear it, so a broken or stale fd is never cached.
    file: Option<Arc<File>>,
    /// Sticky failure from a background `sync_data`, reported by the next [`Self::append_line`].
    /// Typed, not stringified, so `ErrorKind` (ENOSPC) survives the deferral.
    sync_err: Arc<Mutex<Option<std::io::Error>>>,
}
```

`DiskStore::new`'s signature is **unchanged** — the syncer comes from the static — so all
construction sites compile untouched. `rg -n 'DiskStore::new' crates/` finds exactly **6**, all
re-verified at HEAD: [`lifecycle.rs:43,85,116,197`](../../../crates/cyrup-session/src/manager/lifecycle.rs),
[`branched_session.rs:120`](../../../crates/cyrup-session/src/manager/branched_session.rs) and
[`tests/parity.rs:585`](../../../crates/cyrup-session/src/tests/parity.rs).

Both new fields are `Send + Sync`, so `DiskStore` stays `Send` and the `SessionStore: Send` bound
([`store.rs:13`](../../../crates/cyrup-session/src/store.rs)) — which is what lets `SessionManager`
live in the `Arc<AsyncMutex<…>>` at `subscriber.rs:102` — still holds.

### 3.3 The new `append_line`

> **Read the error ordering before you copy this.** The deferred sync error is taken *first* but
> reported *last* — after the write. The obvious spelling (early-`return` the stale error before
> writing) is **wrong**: it drops the current, perfectly good entry in order to report that an
> *earlier* entry's flush failed, turning a power-loss-durability degradation into immediate,
> permanent data loss. Both orderings were built and tested; check-then-write yields `"a\nc\n"`
> (entry `b` gone), write-then-report yields `"a\nb\nc\n"` with the same single error surfaced.

```rust
    fn append_line(&mut self, line: &str) -> Result<(), SessionError> {
        // The `fdatasync` is the only part of this write that is no longer synchronous, so this
        // is where its failure re-enters the caller's `Result`. Take-once: reported exactly one
        // time, then cleared. Taken up-front but returned at the BOTTOM — a stale flush error
        // must not cost us this entry (see PERF-004 §3.3).
        let deferred = self.take_sync_error();
        let file = self.handle()?;
        // ONE `write` of `<json>\n` — R-04-032. The buffer is assembled first *precisely* so this
        // is a single `write(2)`: `O_APPEND` atomicity is per-call, and that is what bounds a
        // crash (or a concurrent appender) to at most one partial final line, which the tolerant
        // reader drops. Before PERF-004 this was two `write_all` calls and the invariant did not
        // actually hold.
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        let mut w: &File = &file;
        if let Err(e) = w.write_all(&buf) {
            self.file = None; // never cache a broken fd; the next append reopens
            return Err(e.into());
        }
        // The bytes are the kernel's now — visible to every reader, and safe against a process
        // crash. Only the device flush is deferred.
        SESSION_SYNCER.request(&file, &self.sync_err);
        match deferred {
            Some(e) => Err(SessionError::Io(e)),
            None => Ok(()),
        }
    }
```

with

```rust
    fn handle(&mut self) -> Result<Arc<File>, SessionError> {
        if let Some(f) = &self.file {
            return Ok(Arc::clone(f));
        }
        let f = Arc::new(OpenOptions::new().create(true).append(true).open(&self.path)?);
        self.file = Some(Arc::clone(&f));
        Ok(f)
    }

    fn take_sync_error(&self) -> Option<std::io::Error> {
        self.sync_err.lock().unwrap_or_else(|p| p.into_inner()).take()
    }
```

`SessionError::Io(#[from] std::io::Error)` is the existing variant
([`error.rs:12-13`](../../../crates/cyrup-session/src/error.rs)) — no new variant is needed, and the
typed slot now hands it a real `io::Error` rather than a reconstructed one. The `flush()` call is
dropped; it was a no-op on a bare `File` and is now provably one.

### 3.4 Invalidate the handle where the inode changes

**This is the one place where holding the fd introduces a bug if you skip it.** `rewrite` writes a
temp sibling and `rename`s over the target
([`store.rs:77-91`](../../../crates/cyrup-session/src/store.rs)) — after that, a cached `file` points
at the **old, unlinked inode**, and every subsequent append writes into a file nobody can read. Add
`self.file = None;` at the top of both `rewrite` and `create_exclusive` (the latter defensively — it
creates a fresh inode).

> **⚠️ This failure is SILENT, and it was re-demonstrated this pass, not argued.** Building the §3
> code with the `self.file = None` removed from `rewrite` only, then running
> `append("old") → rewrite("hdr\n") → append("new")`:
>
> ```
> assertion `left == right` failed: append went to the unlinked inode
>   left: "hdr\n"
>  right: "hdr\nnew\n"
> ```
>
> The second `append_line` returned **`Ok(())`**. No error is raised anywhere — the write succeeds
> against a valid fd whose inode has no directory entry, so the bytes are written and then destroyed
> when the last handle closes. Restoring the one line makes the test pass. This is the single
> highest-risk line in the change.

> **⚠️ CORRECTION carried forward — an earlier revision justified this wrongly.** It claimed the bug
> is latent "because `flushed` only ever goes `false → true`". **That is false.**
> [`branched_session.rs:147`](../../../crates/cyrup-session/src/manager/branched_session.rs) assigns
> `self.flushed = flushed;` from a value computed a few lines earlier, which is `false` whenever the
> retained branch has no assistant message — so a manager that was `flushed == true` can become
> `flushed == false`. Do not rely on that flag for anything here.
>
> **The real invariant is construction order, and it is the one to state in the comment:** every
> `rewrite` call site operates on a store constructed a few lines earlier and **never yet appended
> to** — [`lifecycle.rs:85`](../../../crates/cyrup-session/src/manager/lifecycle.rs) → `rewrite` at
> `:96`, [`lifecycle.rs:116`](../../../crates/cyrup-session/src/manager/lifecycle.rs) → `rewrite` at
> `:119`, and [`branched_session.rs:120`](../../../crates/cyrup-session/src/manager/branched_session.rs)
> → `rewrite` at `:126` (all six line numbers re-verified this pass). So no `rewrite` currently
> follows an `append_line` on the same instance, and the stale fd cannot be observed *by today's call
> graph*. **Do not rely on that staying true** — it is an accident of three call sites, not an
> enforced property. The invalidation is mandatory, not optional.

Also add:

```rust
impl Drop for DiskStore {
    /// One last flush of whatever this store wrote. Non-blocking: the syncer holds its own
    /// `Arc<File>` clone, so the fd outlives this `DiskStore` exactly long enough to be flushed.
    /// This is the branch path — `adopt_branch` replaces `self.store`
    /// (`manager/branched_session.rs:145`), dropping the outgoing store here.
    fn drop(&mut self) {
        if let Some(f) = &self.file {
            SESSION_SYNCER.request(f, &self.sync_err);
        }
    }
}
```

Adding `Drop` is safe here, and this was checked rather than assumed: nothing in the workspace
destructures or partially moves out of a `DiskStore` — it is only ever constructed by
`DiskStore::new` and used behind `Box<dyn SessionStore>` — so the "cannot move out of a type that
implements Drop" rule cannot bite. (`LazyLock` statics are never dropped, so touching
`SESSION_SYNCER` from a destructor at teardown is also well-defined.)

### 3.5 The shutdown barrier — and the two call sites that cannot see it

Export from [`store.rs`](../../../crates/cyrup-session/src/store.rs) and re-export from
[`lib.rs:57`](../../../crates/cyrup-session/src/lib.rs), which today reads
`pub use store::{DiskStore, MemStore, SessionStore};`:

```rust
/// Block until every `sync_data` requested before this call has completed. Costs one flush
/// round (~200 µs); a no-op when nothing is pending. Under option C this is a *power-loss*
/// guarantee only — the bytes are already in the page cache, so no process-exit path can lose
/// them — which is why it is a courtesy at teardown rather than a correctness requirement.
pub fn flush_session_writes() {
    SESSION_SYNCER.barrier();
}
```

> **🔴 REQUIRED, AND MISSING FROM EVERY PREVIOUS REVISION.** Two of the three call sites below live
> in crates that **have no direct `cyrup-session` dependency** and therefore cannot name
> `cyrup_session::flush_session_writes` at all:
>
> | crate | direct `cyrup-session` dep? | can name `cyrup_session::…`? |
> | --- | --- | --- |
> | `cyrup-session-svc` | ✅ yes (`Cargo.toml`) | ✅ |
> | `cyrup` (bin) | ❌ **no** — only `cyrup-session-svc` (`Cargo.toml:53`) | ❌ |
> | `cyrup-tui` | ❌ **no** — only `cyrup-session-svc` (`Cargo.toml:45`) | ❌ |
>
> **Do not add a dependency edge.** `cyrup-session-svc/src/lib.rs` already carries a re-export block
> built for exactly this, whose own doc comments say *"without a direct `cyrup-session` dependency"*
> three times (`:107`, `:114`, `:118`). Add one line beside them, in the same style:
>
> ```rust
> /// Re-exported so the bin's signal handlers and the TUI's wedge-escalation path can drain the
> /// session fsync queue before a hard `process::exit` without a direct `cyrup-session` dependency.
> pub use cyrup_session::flush_session_writes;
> ```
>
> Sites 2 and 3 then call `cyrup_session_svc::flush_session_writes()`. `signals.rs:75` already has
> `use cyrup_session_svc::{AgentSessionRuntime, AppMode};`, so there it is a one-word addition.

Wire it at the three exit points, in this order of importance:

1. [`session/lifecycle.rs:67 dispose_with`](../../../crates/cyrup-session-svc/src/session/lifecycle.rs) —
   the seam every host teardown funnels through (`dispose` at `:33` delegates to it;
   `runtime.dispose()` at [`runtime.rs:774`](../../../crates/cyrup-session-svc/src/runtime.rs) is
   reached from [`main.rs:681`](../../../crates/cyrup/src/main.rs) on quit *and* on a TUI-loop
   error). It is an `async fn`, so call it as
   `let _ = tokio::task::spawn_blocking(flush_session_writes).await;` — the point of the task is not
   to put file I/O back on a runtime worker. This crate can name it directly.
2. [`signals.rs:218` and `:231`](../../../crates/cyrup/src/signals.rs) — both `std::process::exit(…)`
   calls (exact lines re-verified). `:231` runs immediately after `runtime.dispose().await`, so it is
   covered by (1); `:218` (the repeat-signal watcher, inside a `tokio::spawn`) **is not**. Call the
   barrier synchronously there — it is already a hard-exit path and ~200 µs is free.
3. [`app/input_reader.rs:206`](../../../crates/cyrup-tui/src/app/input_reader.rs) —
   `std::process::exit(130)`, the wedge/panic-presses escalation. Same treatment, via the same
   re-export. Earlier revisions called this one optional "if the import is awkward"; with the
   re-export in place it is neither awkward nor optional — `cyrup-tui` already names
   `cyrup_session_svc` in several modules. Do all three.

### 3.6 Record the durability decision

Extend the module doc at [`store.rs:1-2`](../../../crates/cyrup-session/src/store.rs). It must say,
in this order: that pi's `_persist` uses `appendFileSync` and never fsyncs
(`session-manager.ts:1015-1042`, specifically `:1021` and `:1040`); that cyrup deliberately keeps a
`fdatasync` on top of that, giving power-loss durability pi does not have; that the flush is
performed off-thread by `SESSION_SYNCER` so it costs the turn ~1.5 µs instead of ~214 µs; and — the
part that stops the next reader from "simplifying" it — that the `write` itself stays synchronous
**on purpose**, because moving it would put the bytes in a userspace queue and lose them to a process
crash, which is *weaker* than pi rather than stronger.

(Per §0d the `pi` tree is not in this container. Restate the citation from this file; do not go
looking for it, and do not soften the comment because you could not re-open it.)

---

## 4. What must not change

1. **`create_exclusive` and `rewrite` stay fully synchronous, sync included.** They are the
   first-flush and migration paths, they run once or twice per session, and both callers need the
   result before proceeding ([`manager/mod.rs:147-161`](../../../crates/cyrup-session/src/manager/mod.rs),
   [`lifecycle.rs:85,116,197`](../../../crates/cyrup-session/src/manager/lifecycle.rs)). The only
   edit either receives is the §3.4 `self.file = None`.
2. **`MemStore` is untouched.** All **five** of its trait methods stay as they are — `path` → `None`,
   `is_persisted` → `false`, and `append_line`/`rewrite`/`create_exclusive` → `Ok(())`
   ([`store.rs:131-159`](../../../crates/cyrup-session/src/store.rs)).
3. **The `SessionStore` trait signature is unchanged** — no new methods, no `async`. It is `pub` from
   [`lib.rs:57`](../../../crates/cyrup-session/src/lib.rs). `rg 'impl SessionStore'` finds **no**
   implementor outside `store.rs` (only `:42` `DiskStore` and `:131` `MemStore`), so this is a
   public-API constraint rather than a "you will break another impl" one — but it is still the
   difference between a contained change and a cross-crate one.
4. **No new dependency in any `Cargo.toml`.** Not in `cyrup-session`, and — per §3.5 — not in `cyrup`
   or `cyrup-tui` either. The re-export is the mechanism.
5. **`#![forbid(unsafe_code)]`** ([`lib.rs:15`](../../../crates/cyrup-session/src/lib.rs)) stays;
   nothing here needs `unsafe` (`Arc::as_ptr(…) as usize` is a safe operation, and the reference
   implementation compiles under `forbid`).
6. **The `rewrite` temp+rename `[CYRUP-DELTA]` block** ([`store.rs:66-76`](../../../crates/cyrup-session/src/store.rs))
   stays verbatim. (Out of scope but worth knowing: that path syncs the temp file and not the parent
   *directory*, so the rename itself is not power-loss durable. Do not fix it here.)

---

## 5. Hazards worth knowing before you start

- **🆕 Holding the fd changes what happens when a session file is deleted underneath a live manager.**
  Today's reopen-per-append silently **recreates** a deleted session file — producing a headerless
  stub. With a held fd the appends follow the unlinked inode and vanish instead. **The new behaviour
  is the better one** (the file stays deleted, as the user asked), and the suite already exercises
  the scenario: `fork_parent_and_unsaved_guard.rs:147` deletes a live session's file and asserts the
  subsequent fork reports `SessionNotSaved` — an assertion the held fd makes *more* robust, not less.
  The delete path itself guards the active session
  ([`session/files.rs:53`](../../../crates/cyrup-session-svc/src/session/files.rs), *"refusing to
  delete the active session"*), though that guard reads the path through a `try_lock` (`:87`) and so
  is skipped when the manager lock is contended — a pre-existing race this change does not widen.
  Note the decision in the §3.2 field comment; do not add recreate-on-delete behaviour back.
- **fd budget is a non-issue.** One held fd per live `DiskStore`; there is one per `SessionManager`,
  plus short-lived throwaways that are explicitly dropped
  ([`fork_context.rs:495`](../../../crates/cyrup-ext-subagents/src/fork_context.rs) calls
  `drop(throwaway)`). `listing.rs` creates no stores. The syncer holds `Arc` clones only for the
  duration of a round.
- **`TempDir` teardown racing a queued sync is harmless.** A test's `tempfile::TempDir` may unlink the
  session file while the syncer still holds an fd; `fsync` on an unlinked-but-open inode is
  well-defined on Linux and simply succeeds.
- **Clippy**: the deny set is `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`
  (`Cargo.toml:97-101`). The code above uses none of them — `unwrap_or_else(|p| p.into_inner())` is
  the sanctioned poison handling, and every `send`/`recv` result is explicitly discarded or matched.
  `return_self_not_must_use = "warn"` (`:102`) does not apply to `Syncer::start` (it takes no `self`).
  **The one lint that DOES fire is `type_complexity`** — see §3.1; warn-only, but the gate expects
  zero warnings.
- **A sync error with no subsequent append is dropped.** `take_sync_error` surfaces through the *next*
  `append_line`; if a session's last write is the one that fails to flush, nobody hears. The `Drop`
  impl does not report it either (there is no `Result` to report into). Accept this: under option C a
  lost sync error costs power-loss durability on the final entry only, and `cyrup-session` has no
  logging dependency to report it through (§3).
- **`let mut w: &File = &file;`** relies on deref coercion from `&Arc<File>` and on
  `impl Write for &File`; `w` must be `mut` because `Write::write_all` takes `&mut self` (the
  mutability is of the `&File` binding, not of the file). This compiles — confirmed, not assumed.

---

## 6. Order of work

§3 is self-contained and verified; transcribe from it, adapting nothing but the surrounding
`SessionError`/`SessionStore`/`Entry`/`SessionHeader` types, which are already the real ones.

1. Imports (§3.0), then `SyncReq` + `Syncer` + `SESSION_SYNCER` + `PendingSync` + `run` (§3.1),
   including the `tx: None` inline-sync fallback and the send-failure fallback. **Use the
   `PendingSync` alias** or clippy warns.
2. `DiskStore` fields + `handle()` + `take_sync_error()` (§3.2) — the error slot is
   `Arc<Mutex<Option<std::io::Error>>>`, not `String`.
3. Rewrite `append_line` (§3.3) — single buffered write, held fd, deferred sync, sticky error
   **reported after the write, not before**.
4. `self.file = None` in `rewrite`/`create_exclusive`, and `impl Drop` (§3.4). **Do not skip —
   skipping it loses appends silently, demonstrated in §3.4.**
5. `flush_session_writes` + the `lib.rs:57` re-export + **the `cyrup-session-svc` re-export** +
   the three call sites (§3.5). Sites 2 and 3 do not compile without that re-export.
6. The module doc comment (§3.6).
7. `cargo test -p cyrup-session`, then the full gate (DoD 9).

---

## 7. Measurement

### 7.1 The syscall-level probe

Run this **on a real block device**, never tmpfs — see §0b. Kept at
[`tmp/perf004-verify/probe.py`](../../../tmp/perf004-verify/probe.py) this pass.

```python
import os, time, tempfile, shutil
d = tempfile.mkdtemp(dir=os.path.expanduser("~"))   # confirm with `df -T "$HOME"` first
p = os.path.join(d, "s.jsonl")
line = b'{"x":"' + b'y'*400 + b'"}\n'

def bench(label, body, n=300):
    open(p, "wb").close()
    fd = os.open(p, os.O_WRONLY | os.O_APPEND | os.O_CREAT)
    t = time.perf_counter()
    body(fd, n)
    us = (time.perf_counter() - t) / n * 1e6
    os.close(fd)
    print(f"{label:34}: {us:9.2f} us/entry")

def reopen(fd, n):
    for _ in range(n):
        f = os.open(p, os.O_WRONLY | os.O_APPEND | os.O_CREAT)
        os.write(f, line); os.close(f)

bench("write only (1 write)",        lambda fd, n: [os.write(fd, line) for _ in range(n)])
bench("write only (2 writes: HEAD)", lambda fd, n: [(os.write(fd, line[:-1]), os.write(fd, b"\n")) for _ in range(n)])
bench("write + fdatasync each",      lambda fd, n: [(os.write(fd, line), os.fdatasync(fd)) for _ in range(n)])
bench("write + fdatasync every 8",   lambda fd, n: [(os.write(fd, line), os.fdatasync(fd) if (i+1) % 8 == 0 else None) for i in range(n)])
bench("fdatasync, nothing dirty",    lambda fd, n: [os.fdatasync(fd) for _ in range(n)])
bench("reopen+write, no sync",       reopen)
shutil.rmtree(d)
```

Three hosts, three sessions. **The flush cost is the only mobile number, and it moves by 6×** —
which is exactly why the ratio, not the absolute, is the claim:

| | `8f49433` (ext4, /dev/root) | `7913760` (ext4, /dev/root) | **`cd16fe9` (ext4, /dev/vda)** |
| --- | --- | --- | --- |
| write only (1 write) | 1.16 | 1.17 | **1.10** |
| write only (2 writes: HEAD) | 2.29 | 2.29 | **2.08** |
| write + fdatasync each | 1137.84 | 1106.45 | **188.37** |
| write + fdatasync every 8 | 161.46 | 140.46 | **29.48** |
| fdatasync, nothing dirty | 35.38 | 36.79 | **49.46** |
| reopen + write, no sync | 3.29 | 3.24 | **3.07** |

**Quote the ratio, not the microseconds.** `fdatasync` is a device property; on the two previous
hosts it was ~950× a bare write, here it is ~170×. Both are "two-to-three orders of magnitude", which
is the claim the task actually rests on, and no plausible device makes a flush comparable to a
page-cache write.

### 7.2 Head-to-head, in Rust, in one process

The probe measures syscalls; this measures the two `append_line` implementations against each other —
today's transcribed verbatim from `store.rs:51-59`, option C as §3 prescribes — same host, same
payload, same process, 500 entries each. Source:
[`tmp/perf004-verify/src/bin/ab.rs`](../../../tmp/perf004-verify/src/bin/ab.rs), `--release`, three runs:

```
  HEAD  (reopen + 2 writes + flush + sync_data):    200.49 us/entry
  OPT-C (held fd + 1 write + queued sync)      :      1.12 us/entry   -> 179.6x
  HEAD                                         :    243.60 us/entry
  OPT-C                                        :      1.07 us/entry   -> 227.4x
  HEAD                                         :    214.46 us/entry
  OPT-C                                        :      1.02 us/entry   -> 209.3x
```

Both files were asserted to hold exactly 500 lines afterwards, so this is not measuring dropped work.

---

## 8. Verification already done — reproduce it, do not redo the thinking

§3's listings were transcribed into a standalone crate carrying `#![forbid(unsafe_code)]` and the
workspace's exact `[lints.clippy]` deny set, with stand-in `SessionError`/`Entry`/`SessionHeader` so
it builds with **zero dependencies**. This pass, on this host:

```
cargo build                -> exit 0
cargo clippy --all-targets -> exit 0, ZERO warnings   (with the PendingSync alias; see §3.1)
cargo test                 -> 7 passed; 0 failed
caller cost                -> 1.50 us/entry over 2000 real append_line calls
```

The seven tests, and the DoD clause each discharges:

| test | proves | DoD |
| --- | --- | --- |
| `read_after_write_is_immediate_and_ordered` | 200 appends, each re-read from a fresh `open` immediately after; count and order exact | 3, 4 |
| `append_is_one_write_syscall_worth_of_bytes` | `{"a":1}\n{"b":2}\n` — no doubled newline | 6 |
| `concurrent_appenders_never_tear_a_line` | 8 threads × 250 appends to one path via 8 stores: 2000 lines, none torn | 6 |
| `stale_fd_after_rewrite_lands_in_the_named_file` | append-after-`rewrite` reaches the live inode; **fails loudly without §3.4** | 8 |
| `sync_error_surfaces_on_the_next_append_then_clears` | deferred error reported once then cleared, the entry still persisted, **and `ErrorKind::StorageFull` survives** | 5 |
| `barrier_waits_for_queued_flushes` | `flush_session_writes()` returns only after the queue drains | 7 |
| `caller_cost_per_entry_is_microseconds_not_milliseconds` | **1.50 µs/entry** over 2000 appends, asserted `< 10` | 1 |

**Both counterfactuals were re-run this pass and both reproduce:**

- removing `self.file = None` from `rewrite` → `stale_fd_…` fails with
  `left: "hdr\n"` / `right: "hdr\nnew\n"`, while `append_line` returned `Ok(())`;
- inlining the `PendingSync` tuple → clippy emits *"very complex type used"*.

These seven are **not** deliverables — this task adds no test files. They exist so that whoever
implements it can diff their `store.rs` against a shape already known to work. To regenerate the
harness: a two-file crate (`Cargo.toml` with the four deny lints, `src/main.rs` = §3 verbatim plus
the stand-in types) rebuilt in `tmp/`, which is gitignored and will not survive the container — which
is precisely why §3 is authoritative and the harness is not.

---

## 9. Definition of Done

No new tests, benchmarks or documentation files are required. The existing suite is the gate.

1. **A persisted entry costs the caller <10 µs**, measured on a real block device (not tmpfs — §0b),
   down from ~214 µs. Measured at **1.50 µs** on the reference implementation (§8) and **1.02-1.12 µs**
   head-to-head (§7.2).
2. **Durability is not reduced on any axis.** When `append_line` returns `Ok`, the bytes are in the
   page cache, so a `SIGKILL`, an `abort`, or a `std::process::exit` loses nothing — matching pi
   exactly. The device flush follows within one syncer round, preserving the power-loss guarantee
   cyrup has today and pi does not.
3. **Ordering is exact**, including across a rapid burst and across two managers writing different
   files — which holds by construction, since the write never leaves the caller's thread.
4. **Read-after-write is unchanged.** A reader that opens the file the instant `append_line` returns
   sees the entry. Specifically:
   [`fork_context.rs:491`](../../../crates/cyrup-ext-subagents/src/fork_context.rs) still forks a
   complete parent transcript, both rename paths
   ([`session/files.rs:79`](../../../crates/cyrup-session-svc/src/session/files.rs) and `:160`) still
   see the live tail, and the `/resume` listing
   ([`listing.rs:55`](../../../crates/cyrup-session/src/listing.rs)) still reports a live session's
   current message count.
5. **A write failure is still visible synchronously, with its kind intact.** ENOSPC/EACCES from the
   `write` returns from `append_line` exactly as today. A *sync* failure is reported by the next
   `append_line` (§3.3) rather than being dropped — **that entry is still written**, because the error
   is returned after the write, and the `io::Error` is carried through untouched so `ErrorKind` still
   identifies ENOSPC. (The workspace has hit ENOSPC mid-run before; this path must not go quiet, and
   must not go unrecognisable, when it happens again.)
6. **The tolerant-reader invariant is stronger than before, not weaker.** The append is now genuinely
   one `write(2)`, so at most one partial final line is possible — R-04-032, `store.rs:52-53`, which
   becomes an accurate comment.
7. **No tokio worker is occupied by file I/O.** The syncer is one OS thread; `append_line` uses no
   `spawn_blocking`; the only `spawn_blocking` is the single teardown barrier (§3.5).
8. **A stale fd is impossible after a `rewrite`.** `rewrite`'s rename is followed by an append that
   lands in the file the path names — the §3.4 invalidation. Note this failure returns `Ok(())` when
   it is wrong, so it must be checked by **reading the file back**, not by checking a result.
9. **All three barrier call sites compile and are wired** (§3.5), including the `cyrup-session-svc`
   re-export that sites 2 and 3 need, and **no `Cargo.toml` gained a dependency**.
10. **The suite is green under the real gate:**
    `cargo test --workspace --features test-fixtures --no-fail-fast`, and
    `cargo clippy --workspace --all-targets --features test-fixtures` exits **0** (the no-panic lints
    do **not** fire under `cargo build`/`cargo test`; check the exit code, not the output). Zero
    *warnings* too, not just zero errors — `type_complexity` (§3.1) is the one this change can
    realistically trip. Note `cyrup-tui` has no `test-fixtures` feature; use `-p cyrup-tui --all-targets`.
