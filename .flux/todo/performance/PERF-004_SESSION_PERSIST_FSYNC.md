---
stage: aug
status: done
updated: 2026-08-29 16:56
---

# Get the per-entry `fdatasync` off the session-write path

> **Measured: ~1106 µs per persisted entry vs 1.17 µs for the write alone — 946×.** On ext4
> (`/dev/root`, the volume `$HOME` lives on). Every persisted session entry pays a full device
> flush, synchronously, on a tokio worker thread, while holding the session mutex.
>
> **The fix is measured too: 1.72 µs/entry through the real `append_line`**, running the §3 code
> as written (§8). That is a **643×** reduction, with every durability guarantee intact.

---

## 0. READ THIS FIRST — three traps, two of which will make you build the wrong thing

### Verification stamp (re-audited at `7913760`, 2026-08-29 16:56)

Every file:line, every upstream citation and every number below was re-checked against the working
tree at HEAD `7913760` (the branch moved on from `8f49433`; `store.rs` itself is byte-identical).
The measurements were independently re-run on this host (§7) and reproduce within noise.

**This revision does something the previous ones did not: the prescribed code was actually
compiled, clippy'd and run.** A standalone crate carrying the same `#![forbid(unsafe_code)]` and
the same no-panic deny set was built from §3's listings verbatim; see
[`tmp/perf004-verify/store-option-c-verified.rs`](../../../tmp/perf004-verify/store-option-c-verified.rs).
That exercise found **two real defects in the prescribed code** and proved one hazard is not
hypothetical:

1. **§3.1 as previously written breaks the clippy gate.** `clippy::type_complexity` fires on
   `Vec<(Arc<File>, Arc<Mutex<Option<String>>>)>`. It is warn-by-default, and this workspace's
   steady state is *zero* warnings — so the code as specified would have been committed and then
   discovered by DoD 9. Fixed with a `type PendingSync` alias (§3.1).
2. **§3.3's error ordering caused avoidable data loss.** Checking the deferred sync error *before*
   the write means the entry that reports a *previous* flush failure is itself never written —
   converting a past power-loss-durability degradation into present, permanent loss of a good
   entry. Corrected to write-then-report (§3.3), which persists everything *and* still surfaces
   the error exactly once. Proven both ways.
3. **§3.4's stale fd is silent data loss, and is now demonstrated.** With the `self.file = None`
   removed from `rewrite`, the follow-up append returns `Ok(())` and the file reads `"hdr\n"`
   instead of `"hdr\nnew\n"`. No error, anywhere. See §3.4.

One claim in an earlier revision was false and remains corrected in §3.4 (the `flushed` flag
rationale). Read that correction before you write any code.

**(a) Measuring this on `/tmp` reports NO DIFFERENCE, and `/tmp` here is tmpfs.**

```
  /tmp  (tmpfs, RAM-backed):  write 0.8 us | write+fdatasync    1.3 us  ->    2x
  $HOME (ext4, /dev/root)  :  write 1.2 us | write+fdatasync 1106.5 us ->  946x
```

`df -T` confirms it: `/` is `ext4` on `/dev/root` (96 G), `/tmp` is a separate 16 G `tmpfs`.
**Measure on the real filesystem the session file actually lives on.** The probe is in §7.

**(b) The recommendation in the original draft of this task was WRONG. Do not implement it.**

That draft recommended *"keep the sync, move the whole write to a background writer thread fed by
an unbounded channel"* (option B below). B **weakens durability below pi's**, which is the exact
opposite of what it claims, and it breaks read-after-write for **five** production call sites and a
dozen tests. The reasoning is in §2 and it is the single most important thing on this page. The
correct answer is **option C**, it is smaller than B, and it needs no fence, no queue, no shutdown
protocol and no error-routing plumbing.

**(c) This is not a straight parity gap — cyrup is doing something pi does not.** pi persists with
`appendFileSync` ([`session-manager.ts:1021,1040`](../../../../pi/packages/coding-agent/src/core/session-manager.ts)),
inside `_persist` (`:1015-1042`), which is `open(O_APPEND)` + `write` + `close` and **never** calls
`fsync`/`fdatasync`. Verified exhaustively: `grep -n 'fsync\|fdatasync' session-manager.ts` returns
**nothing**; the only write primitives in the file are `appendFileSync` / `writeFileSync`
(`:984`, `:1021`, `:1033`, `:1040`, `:1620`, `:1625`). cyrup's
[`store.rs:58`](../../../crates/cyrup-session/src/store.rs) adds `f.sync_data()?`. So cyrup offers a
**strictly stronger** guarantee than pi and pays ~980× per entry for it. Option C keeps that
guarantee and pays ~1.2 µs.

---

## 1. Where it is

[`crates/cyrup-session/src/store.rs:51-59`](../../../crates/cyrup-session/src/store.rs) (line
numbers exact at HEAD):

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

1. **`sync_data()` — ~1106 µs of the ~1110.** The whole problem.
2. **`OpenOptions::…open()` per call.** The fd is not held: an `open`+`close` pair per entry where
   one long-lived `O_APPEND` fd would do. Measured cost of the reopen: **3.24 µs vs 1.17 µs held**,
   i.e. the reopen is ~3× the entire cost of the write it wraps.
3. **The comment at `:52-53` is false of the code at `:54-55`.** It claims "One `write` of
   `<json>\n`" — the code issues **two** `write(2)` calls, payload then newline. `O_APPEND`
   atomicity is *per `write` call*, so the invariant the comment asserts (and that R-04-032's
   tolerant reader is specified against) does not actually hold: a concurrent appender on the same
   file can interleave between the two, producing `{"a":1}{"b":2}\n\n`. Coalescing into one buffer
   is free — it is also **1.12 µs/entry faster** (2.29 → 1.17) — and makes the comment true. §8
   includes a concurrency test that drives 8 threads × 250 appends through one path and asserts no
   line tears; it passes against the §3 code.
4. **`f.flush()` is a no-op** on a bare `File` (no `BufWriter`). Harmless, but it is dead code that
   reads as if it does something.
5. **It is synchronous inside an async lock, on a tokio worker.** The caller chain, verified
   end-to-end:

   [`subscriber.rs:171`](../../../crates/cyrup-session-svc/src/subscriber.rs) —
   `let _ = self.manager.lock().await.append_message(core);` — inside
   `impl EventSubscriber for SvcSubscriber::on_event`, an `async fn` on a
   `#[tokio::main(flavor = "multi_thread")]` runtime
   ([`main.rs:48`](../../../crates/cyrup/src/main.rs))
   → [`manager/append.rs:24`](../../../crates/cyrup-session/src/manager/append.rs) `append_agent_message`
   → [`manager/mod.rs:127`](../../../crates/cyrup-session/src/manager/mod.rs) `push_entry`
   → [`manager/mod.rs:143`](../../../crates/cyrup-session/src/manager/mod.rs) `persist_last()?`
   → [`manager/mod.rs:147-161`](../../../crates/cyrup-session/src/manager/mod.rs) `persist_last`
   → `store.append_line` (`:154`) — which `rg -n append_line crates/` confirms is its **only**
   production caller in the workspace.

   So each entry blocks **a runtime worker for ~1.11 ms** *and* holds
   `Arc<AsyncMutex<SessionManager>>` ([`subscriber.rs:102`](../../../crates/cyrup-session-svc/src/subscriber.rs))
   for that whole time — serializing every other manager access (`session_file()`, fork resolution,
   `list_sessions`, the TUI's tree reads) behind a device flush. A tool-using turn persists
   `3 + N` entries (user, assistant-with-calls, one per tool result, final assistant), so a 5-call
   turn stalls ≈ **8.9 ms** inside the lock.

**Note what is already lost:** `subscriber.rs:171` and `:177-184` both discard the `Result`
(`let _ = …`), and `:171` is the **only** `append_message` call site in `cyrup-session-svc` or
`cyrup-tui` (`rg -n 'append_message\(' crates/cyrup-session-svc/src crates/cyrup-tui/src`). The
highest-volume caller of `append_line` **already** swallows every persistence error today. That is
relevant to DoD 5 — it is a pre-existing hole, and option C is the only option here that does not
widen it. It is also why §3.3's write-then-report ordering is safe: an `Err` returned *after* a
successful write cannot mislead a caller that never reads it.

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
| **cyrup today** | on the **device** | ✅ yes | ✅ yes | **~1110 µs** |
| **A.** drop `sync_data()` | in the page cache | ✅ yes | ❌ no | 1.2 µs |
| **B.** async writer thread | in a **userspace queue** | ❌ **NO** | ❌ no (until drained) | ~0.5 µs |
| **C.** sync write, async sync | in the **page cache** | ✅ yes | ✅ yes, after ≤1 round | **1.72 µs (measured, §8)** |

Read the `SIGKILL` column. **Option B is the only row that loses data to a process crash** — the
queue dies with the process. B is not "the same durability, moved off the caller"; it is
*weaker than pi*, in exchange for 0.7 µs over option C. Everything B then has to build —
the drain-at-shutdown protocol, the error re-routing, the unbounded channel — exists solely to
partially claw back a guarantee it just gave away.

The page cache is the key fact: **`write(2)` hands ownership of the bytes to the kernel.** The
process can be `SIGKILL`ed, `abort()`ed, or `std::process::exit`ed and the data still reaches the
disk, because the kernel is what writes it back. `fdatasync` only adds *power-loss* durability. So
splitting the two — keep the `write` synchronous, move only the `fdatasync` — keeps every
guarantee cyrup has today and pays 1.2 µs for it.

**C also costs nothing to prove correct**, which B cannot claim:

- **Read-after-write is preserved for free.** The page cache is coherent: any reader — including a
  *different process* — sees the bytes the instant `write` returns. Under B, **five** production
  sites read a live session's file and would see a stale tail (all five re-verified at HEAD):
  - [`fork_context.rs:491`](../../../crates/cyrup-ext-subagents/src/fork_context.rs) —
    `SessionManager::open(&persisted_path)?` re-reads the **live parent's file from disk** to build
    a subagent's forked branch (the code even documents it as a "THROWAWAY handle on the parent's
    PERSISTED file", `:487`). A queued tail means the subagent silently starts from a truncated
    transcript.
  - [`session/files.rs:79`](../../../crates/cyrup-session-svc/src/session/files.rs) —
    `SessionManager::open(path)?` then `append_session_info` on **another** session's file
    (`rename_session_file`, `:69`).
  - [`session/files.rs:160`](../../../crates/cyrup-session-svc/src/session/files.rs) —
    `rename_session_file_at`, the same two calls made by the **pre-launch** `--resume` picker
    before any session service exists (SEAM-062). *This site was missing from earlier revisions.*
  - [`runtime.rs:606`](../../../crates/cyrup-session-svc/src/runtime.rs) — `SessionManager::open(&file)?`
    on a session switch/branch.
  - [`listing.rs:55`](../../../crates/cyrup-session/src/listing.rs) — `list_in_dir` streams every
    file for the `/resume` picker's titles and message counts.
- **Ordering is exact by construction.** Writes stay on the caller's thread behind `&mut self`;
  there is no second writer to order against. §8 asserts this over 200 sequential appends and over
  8 concurrent threads.
- **No test changes.** At least a dozen tests append and then immediately read the file — e.g.
  [`tests/sessions.rs:71,116`](../../../crates/cyrup-session/src/tests/sessions.rs)
  (`std::fs::read_to_string(&file)` immediately after the append that creates it), and
  [`tests/parity.rs:585-593`](../../../crates/cyrup-session/src/tests/parity.rs), which drives a
  bare `DiskStore` directly. Under B each needs a fence; under C each is untouched.
- **The write error stays synchronous.** ENOSPC/EACCES/EDQUOT still return from `append_line`
  exactly as today. Only the *sync* error is deferred, and §3.3 routes it back into the same
  `Result` on the next call.
- **Shutdown stops being a correctness requirement.** Under B, missing a drain loses user data.
  Under C, missing the final sync degrades power-loss durability for the last few entries and
  nothing else. The barrier in §3.5 is still worth wiring; it is no longer load-bearing.

**Decision: implement option C.** Record the reasoning in the `store.rs` doc comment (§3.6) so the
next reader does not re-litigate it — including the fact that cyrup deliberately fsyncs where pi
does not.

### Why the batched sync needs no timer

Measured on this host, ext4 (§7 for the probe; these are the fresh numbers):

```
  write only (held fd, one write)  :     1.17 us/entry
  write only (held fd, two writes) :     2.29 us/entry     <- what the code does today
  reopen + write, no sync          :     3.24 us/entry     <- the per-call open+close today
  write + fdatasync every entry    :  1106.45 us/entry     <- what the code does today
  write + fdatasync every 8        :   140.46 us/entry
  fdatasync with nothing dirty     :    36.79 us/call
```

A background syncer that simply **drains everything queued behind the message that woke it, dedups,
and syncs once** self-tunes with no configuration: at one entry per turn it syncs once per entry
(off-thread, invisible); during a burst, every entry that arrives inside the ~1.1 ms flush collapses
into the *next* single flush. Adding a debounce timer would only make the quiet case worse. The
`36.79 µs` row is why the dedup is worth having rather than syncing per queued message.

---

## 3. Required implementation (option C)

All of it is in [`crates/cyrup-session/src/store.rs`](../../../crates/cyrup-session/src/store.rs)
except §3.5. **No dependency is added** — `std::sync::mpsc`, `LazyLock` and `std::thread` are
enough, and all three are available at the workspace's `rust-version = "1.96"` (`LazyLock`
stabilised 1.80; `io::Error::other` 1.74). `cyrup-session` has **no logging dependency** — verified:
`rg 'tracing|log::|eprintln|println!' crates/cyrup-session/src` matches **zero** files — and must
not gain one; the error path in §3.3 is built on the existing `Result` for that reason.

> **The listings in §3.0–3.4 are the exact text that was compiled, clippy'd (exit 0, zero warnings)
> and run under seven behavioural tests.** The verified source is
> [`tmp/perf004-verify/store-option-c-verified.rs`](../../../tmp/perf004-verify/store-option-c-verified.rs).
> Transcribe from there; do not re-derive.

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
this decision — read its comment before writing yours. (`LazyLock` is already used in 11 files
across the workspace; `std::thread::Builder::new()` in **4** — `cyrup-provider/src/auth/oauth/callback.rs`,
`cyrup-tui/src/{clipboard,open_browser}.rs`, and
[`spawn/signal.rs:603`](../../../crates/cyrup-ext-subagents/src/spawn/signal.rs), the last of which
is `#[cfg(all(test, unix))]`, so the three production precedents are the first three.)

Global, not per-`DiskStore`, because store instances are not stable: `adopt_branch`
**replaces** `self.store` wholesale
([`branched_session.rs:145`](../../../crates/cyrup-session/src/manager/branched_session.rs)),
`fork_context` opens a throwaway `SessionManager` on a live file, and `rename_session_file` opens a
second manager on another file. One thread for the process bounds the cost at one OS thread total
and gives a single total order over every session write in the process.

```rust
/// A background `fdatasync` request: the handle to flush, plus the slot its failure is
/// reported through (see `DiskStore::sync_err`).
enum SyncReq {
    Sync { file: Arc<File>, err: Arc<Mutex<Option<String>>> },
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

`Syncer::start` matches on the spawn `Result` — **no `unwrap`/`expect`** (`[workspace.lints.clippy]`
denies `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`; they fire only under
`cargo clippy`). On `Err`, return `Syncer { tx: None }`.

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

    fn request(&self, file: &Arc<File>, err: &Arc<Mutex<Option<String>>>) {
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

    fn sync_inline(file: &Arc<File>, err: &Arc<Mutex<Option<String>>>) {
        if let Err(e) = file.sync_data() {
            let mut slot = err.lock().unwrap_or_else(|p| p.into_inner());
            *slot = Some(e.to_string());
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
Spelling the vec `Vec<(Arc<File>, Arc<Mutex<Option<String>>>)>` inline trips
`clippy::type_complexity`, which is warn-by-default and would break this workspace's zero-warning
steady state (DoD 9). Confirmed by compiling both forms.

```rust
/// One queued flush: the handle, plus the slot its failure is reported through. Named because
/// `clippy::type_complexity` fires on the bare tuple below and the gate expects zero warnings.
type PendingSync = (Arc<File>, Arc<Mutex<Option<String>>>);

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
                *slot = Some(e.to_string());
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
    sync_err: Arc<Mutex<Option<String>>>,
}
```

`DiskStore::new`'s signature is **unchanged** — the syncer comes from the static — so all five
construction sites
([`lifecycle.rs:43,85,116,197`](../../../crates/cyrup-session/src/manager/lifecycle.rs),
[`branched_session.rs:120`](../../../crates/cyrup-session/src/manager/branched_session.rs)) and
[`tests/parity.rs:585`](../../../crates/cyrup-session/src/tests/parity.rs) compile untouched. Those
six are the complete set (`rg -n 'DiskStore::new' crates/`).

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
        // must not cost us this entry (see the note in PERF-004 §3.3).
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
            Some(msg) => Err(SessionError::Io(std::io::Error::other(msg))),
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

    fn take_sync_error(&self) -> Option<String> {
        self.sync_err.lock().unwrap_or_else(|p| p.into_inner()).take()
    }
```

`SessionError::Io(#[from] std::io::Error)` is the existing variant
([`error.rs:12-13`](../../../crates/cyrup-session/src/error.rs)) — no new variant is needed. The
`flush()` call is dropped; it was a no-op on a bare `File` and is now provably one.

### 3.4 Invalidate the handle where the inode changes

**This is the one place where holding the fd introduces a bug if you skip it.** `rewrite` writes a
temp sibling and `rename`s over the target
([`store.rs:77-91`](../../../crates/cyrup-session/src/store.rs)) — after that, a cached `file`
points at the **old, unlinked inode**, and every subsequent append writes into a file nobody can
read. Add `self.file = None;` at the top of both `rewrite` and `create_exclusive` (the latter
defensively — it creates a fresh inode).

> **⚠️ This failure is SILENT, and it has now been demonstrated rather than argued.** Building the
> §3 code with the `self.file = None` removed from `rewrite` only, then running
> `append("old") → rewrite("hdr\n") → append("new")`:
>
> ```
> assertion `left == right` failed: append went to the unlinked inode
>   left: "hdr\n"
>  right: "hdr\nnew\n"
> ```
>
> The second `append_line` returned **`Ok(())`**. No error is raised anywhere — the write succeeds
> against a valid fd whose inode has no directory entry, so the bytes are written and then
> destroyed when the last handle closes. Restoring the one line makes the test pass. This is the
> single highest-risk line in the change; §8's `stale_fd_after_rewrite_lands_in_the_named_file` is
> the regression that catches it.

> **⚠️ CORRECTION carried forward — the previous revision justified this wrongly.**
> It claimed the bug is latent "because `flushed` only ever goes `false → true`
> ([`manager/mod.rs:147-161`](../../../crates/cyrup-session/src/manager/mod.rs))". **That is false.**
> [`adopt_branch`](../../../crates/cyrup-session/src/manager/branched_session.rs) at `:147` assigns
> `self.flushed = flushed;` from a value computed at `:125-130`, which is `false` whenever the
> retained branch has no assistant message — so a manager that was `flushed == true` can and does
> become `flushed == false`. Do not rely on that flag for anything here.
>
> **The real invariant is construction order, and it is the one to state in the comment:** every
> `rewrite` call site operates on a store constructed a few lines earlier and **never yet appended
> to** — [`lifecycle.rs:85`](../../../crates/cyrup-session/src/manager/lifecycle.rs) → `rewrite` at
> `:96`, [`lifecycle.rs:116`](../../../crates/cyrup-session/src/manager/lifecycle.rs) → `rewrite` at
> `:119`, and [`branched_session.rs:120`](../../../crates/cyrup-session/src/manager/branched_session.rs)
> → `rewrite` at `:126`. So no `rewrite` currently follows an `append_line` on the same instance,
> and the stale fd cannot be observed *by today's call graph*. **Do not rely on that staying true**
> — it is an accident of three call sites, not an enforced property, and the demonstration above is
> what the accident is protecting you from. The invalidation is mandatory, not optional.

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

Adding `Drop` is safe here, and this was checked rather than assumed: `rg -n 'DiskStore\s*\{|\.store\b' crates/cyrup-session/src`
shows nothing in the workspace destructures or partially moves out of a `DiskStore` — it is only
ever constructed by `DiskStore::new` and used behind `Box<dyn SessionStore>` — so the "cannot move
out of a type that implements Drop" rule cannot bite. (`LazyLock` statics are never dropped, so
touching `SESSION_SYNCER` from a destructor at teardown is also well-defined.)

### 3.5 The shutdown barrier

Export from [`store.rs`](../../../crates/cyrup-session/src/store.rs) and re-export from
[`lib.rs:57`](../../../crates/cyrup-session/src/lib.rs) alongside `DiskStore`
(`pub use store::{flush_session_writes, DiskStore, MemStore, SessionStore};`):

```rust
/// Block until every `sync_data` requested before this call has completed. Costs one flush
/// round (~1 ms); a no-op when nothing is pending. Under option C this is a *power-loss*
/// guarantee only — the bytes are already in the page cache, so no process-exit path can lose
/// them — which is why it is a courtesy at teardown rather than a correctness requirement.
pub fn flush_session_writes() {
    SESSION_SYNCER.barrier();
}
```

Wire it at the three exit points, in this order of importance:

1. [`session/lifecycle.rs:67 dispose_with`](../../../crates/cyrup-session-svc/src/session/lifecycle.rs) —
   the seam every host teardown funnels through (`dispose` at `:33` delegates to it;
   `runtime.dispose()` at [`runtime.rs:774`](../../../crates/cyrup-session-svc/src/runtime.rs) is
   reached from [`main.rs:681`](../../../crates/cyrup/src/main.rs) on quit *and* on a TUI-loop
   error). It is an `async fn`, so call it as
   `let _ = tokio::task::spawn_blocking(flush_session_writes).await;` — the point of the task is not
   to put file I/O back on a runtime worker.
2. [`signals.rs:218` and `:231`](../../../crates/cyrup/src/signals.rs) — both
   `std::process::exit(…)` calls (exact lines verified). `:231` runs immediately after
   `runtime.dispose().await`, so it is covered by (1); `:218` (the repeat-signal watcher) **is not**
   and calls `exit` directly. Call the barrier synchronously there — it is already a hard-exit path
   and ~1 ms is free.
3. [`app/input_reader.rs:206`](../../../crates/cyrup-tui/src/app/input_reader.rs) —
   `std::process::exit(130)`, the wedge/panic-presses escalation. Same treatment. Note `cyrup-tui`
   already depends on the session layer transitively; if the import is awkward, leaving this one
   is acceptable **because option C loses no data here** — record that in the comment rather than
   adding a dependency edge.

### 3.6 Record the durability decision

Extend the module doc at [`store.rs:1-2`](../../../crates/cyrup-session/src/store.rs). It must say,
in this order: that pi's `_persist` uses `appendFileSync` and never fsyncs
(`session-manager.ts:1015-1042`, specifically `:1021` and `:1040`); that cyrup deliberately keeps a
`fdatasync` on top of that, giving power-loss durability pi does not have; that the flush is
performed off-thread by `SESSION_SYNCER` so it costs the turn ~1.2 µs instead of ~1141 µs; and —
the part that stops the next reader from "simplifying" it — that the `write` itself stays
synchronous **on purpose**, because moving it would put the bytes in a userspace queue and lose
them to a process crash, which is *weaker* than pi rather than stronger.

---

## 4. What must not change

1. **`create_exclusive` and `rewrite` stay fully synchronous, sync included.** They are the
   first-flush and migration paths, they run once or twice per session, and both callers need the
   result before proceeding ([`manager/mod.rs:147-161`](../../../crates/cyrup-session/src/manager/mod.rs),
   [`lifecycle.rs:85,116,197`](../../../crates/cyrup-session/src/manager/lifecycle.rs)).
2. **`MemStore` is untouched.** All **five** of its trait methods stay as they are — `path` → `None`,
   `is_persisted` → `false`, and `append_line`/`rewrite`/`create_exclusive` → `Ok(())`
   ([`store.rs:131-159`](../../../crates/cyrup-session/src/store.rs)). (Earlier revisions said
   "four"; the trait has five.)
3. **The `SessionStore` trait signature is unchanged** — no new methods, no `async`. It is `pub`
   from [`lib.rs:57`](../../../crates/cyrup-session/src/lib.rs). (For accuracy: `rg 'impl SessionStore'`
   finds **no** implementor outside `store.rs` — only `:42` `DiskStore` and `:131` `MemStore` — so
   this is a public-API constraint rather than a "you will break another impl" one, but it is still
   the difference between a contained change and a cross-crate one, and there is no reason to spend
   it.)
4. **No new dependency in `cyrup-session/Cargo.toml`.**
5. **`#![forbid(unsafe_code)]`** ([`lib.rs:15`](../../../crates/cyrup-session/src/lib.rs)) stays;
   nothing here needs `unsafe` (`Arc::as_ptr(…) as usize` is a safe operation).
6. **The `rewrite` temp+rename `[CYRUP-DELTA]` block** ([`store.rs:66-76`](../../../crates/cyrup-session/src/store.rs))
   stays verbatim. (Out of scope but worth knowing: that path syncs the temp file and not the
   parent *directory*, so the rename itself is not power-loss durable. Do not fix it here.)

---

## 5. Hazards worth knowing before you start

- **fd budget is a non-issue.** One held fd per live `DiskStore`; there is one per `SessionManager`,
  plus short-lived throwaways that are explicitly dropped
  ([`fork_context.rs:495`](../../../crates/cyrup-ext-subagents/src/fork_context.rs) calls
  `drop(throwaway)`). `listing.rs` creates no stores. The syncer holds `Arc` clones only for the
  duration of a round.
- **`TempDir` teardown racing a queued sync is harmless.** A test's `tempfile::TempDir` may unlink
  the session file while the syncer still holds an fd; `fsync` on an unlinked-but-open inode is
  well-defined on Linux and simply succeeds.
- **Clippy**: the deny set is `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`
  (`Cargo.toml:98-101 [workspace.lints.clippy]`). The code above uses none of them —
  `unwrap_or_else(|p| p.into_inner())` is the sanctioned poison handling, and every `send`/`recv`
  result is explicitly discarded or matched. `return_self_not_must_use = "warn"` (`:102`) does not
  apply to `Syncer::start` (it takes no `self`). **The one lint that DOES fire is
  `type_complexity`** — see §3.1; it is warn-only but the gate expects zero warnings.
- **A sync error with no subsequent append is dropped.** `take_sync_error` surfaces through the
  *next* `append_line`; if a session's last write is the one that fails to flush, nobody hears. The
  `Drop` impl does not report it either (there is no `Result` to report into). Accept this: under
  option C a lost sync error costs power-loss durability on the final entry only, and
  `cyrup-session` has no logging dependency to report it through (§3).
- **`let mut w: &File = &file;`** relies on deref coercion from `&Arc<File>` and on
  `impl Write for &File`; `w` must be `mut` because `Write::write_all` takes `&mut self` (the
  mutability is of the `&File` binding, not of the file). This compiles — confirmed, not assumed.

---

## 6. Order of work

Transcribe from [`tmp/perf004-verify/store-option-c-verified.rs`](../../../tmp/perf004-verify/store-option-c-verified.rs)
— it is this plan's code, already compiled and tested — adapting names back to the real
`SessionError`/`SessionStore`/`Entry`/`SessionHeader` types.

1. Imports (§3.0), then `SyncReq` + `Syncer` + `SESSION_SYNCER` + `PendingSync` + `run` (§3.1),
   including the `tx: None` inline-sync fallback and the send-failure fallback. **Use the
   `PendingSync` alias** or clippy warns.
2. `DiskStore` fields + `handle()` + `take_sync_error()` (§3.2).
3. Rewrite `append_line` (§3.3) — single buffered write, held fd, deferred sync, sticky error
   **reported after the write, not before**.
4. `self.file = None` in `rewrite`/`create_exclusive`, and `impl Drop` (§3.4). **Do not skip —
   skipping it loses appends silently, demonstrated in §3.4.**
5. `flush_session_writes` + the `lib.rs:57` re-export + the three call sites (§3.5).
6. The module doc comment (§3.6).
7. `cargo test -p cyrup-session`, then the full gate (DoD 9).

---

## 7. The measurement probe

Run this **on ext4** (`$HOME`), never `/tmp` — see §0a.

```python
import os, time, tempfile, shutil
d = tempfile.mkdtemp(dir=os.path.expanduser("~"))   # MUST be the real fs, not /tmp
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

Runs on this host (ext4, `/dev/root`), two independent sessions a day apart:

| | `8f49433` | `7913760` |
| --- | --- | --- |
| write only (1 write) | 1.16 | **1.17** |
| write only (2 writes: HEAD) | 2.29 | **2.29** |
| write + fdatasync each | 1137.84 | **1106.45** |
| write + fdatasync every 8 | 161.46 | **140.46** |
| fdatasync, nothing dirty | 35.38 | **36.79** |
| reopen + write, no sync | 3.29 | **3.24** |

The only mobile number is the flush itself, and it moves by ~3%. The conclusion does not depend on
which run you take.

---

## 8. The reference implementation is already verified — use it

Everything in §3 was transcribed into a standalone crate carrying `#![forbid(unsafe_code)]` and the
workspace's exact `[lints.clippy]` deny set, then built and exercised. Source:
[`tmp/perf004-verify/store-option-c-verified.rs`](../../../tmp/perf004-verify/store-option-c-verified.rs).

```
cargo build          -> exit 0
cargo clippy --all-targets -> exit 0, ZERO warnings   (after the PendingSync alias; see §3.1)
cargo test           -> 7 passed; 0 failed
```

The seven tests, and the DoD clause each one discharges:

| test | proves | DoD |
| --- | --- | --- |
| `read_after_write_is_immediate_and_ordered` | 200 appends, each re-read from a fresh `open` immediately after; count and order exact | 3, 4 |
| `append_is_one_write_syscall_worth_of_bytes` | `{"a":1}\n{"b":2}\n` — no doubled newline | 6 |
| `concurrent_appenders_never_tear_a_line` | 8 threads × 250 appends to one path via 8 stores: 2000 lines, none torn | 6 |
| `stale_fd_after_rewrite_lands_in_the_named_file` | append-after-`rewrite` reaches the live inode; **fails loudly without §3.4** | 8 |
| `sync_error_surfaces_on_the_next_append_then_clears` | deferred error reported once, then cleared, **and the entry still persisted** | 5 |
| `barrier_waits_for_queued_flushes` | `flush_session_writes()` returns only after the queue drains | 7 |
| `caller_cost_per_entry_is_microseconds_not_milliseconds` | **1.72 µs/entry** over 2000 appends, asserted `< 10` | 1 |

**1.72 µs measured through the real `append_line`**, against ~1106 µs today: a **643×** reduction.
The gap between 1.72 and the raw 1.17 µs write is the channel send plus the `Arc` clones — i.e. the
coordination overhead of option C is ~0.55 µs, three orders of magnitude below what it removes.

These seven are **not** required deliverables (DoD adds no new test files). They exist so that
whoever implements this can diff their `store.rs` against a version already known to work, and can
re-run them in `tmp/` if a behaviour is in doubt.

---

## 9. Definition of Done

No new tests, benchmarks or documentation files are required. The existing suite is the gate.

1. **A persisted entry costs the caller <10 µs**, measured on ext4 (not tmpfs — §0a), down from
   ~1106 µs. Measured at **1.72 µs** on the reference implementation (§8).
2. **Durability is not reduced on any axis.** When `append_line` returns `Ok`, the bytes are in the
   page cache, so a `SIGKILL`, an `abort`, or a `std::process::exit` loses nothing — matching pi
   exactly. The device flush follows within one syncer round, preserving the power-loss guarantee
   cyrup has today and pi does not.
3. **Ordering is exact**, including across a rapid burst and across two managers writing different
   files — which holds by construction, since the write never leaves the caller's thread.
   (§8 `read_after_write_is_immediate_and_ordered`, `concurrent_appenders_never_tear_a_line`.)
4. **Read-after-write is unchanged.** A reader that opens the file the instant `append_line`
   returns sees the entry. Specifically:
   [`fork_context.rs:491`](../../../crates/cyrup-ext-subagents/src/fork_context.rs) still forks a
   complete parent transcript, both rename paths
   ([`session/files.rs:79`](../../../crates/cyrup-session-svc/src/session/files.rs) and `:160`)
   still see the live tail, and the `/resume` listing
   ([`listing.rs:55`](../../../crates/cyrup-session/src/listing.rs)) still reports a live session's
   current message count.
5. **A write failure is still visible synchronously.** ENOSPC/EACCES from the `write` returns from
   `append_line` exactly as today. A *sync* failure is reported by the next `append_line` (§3.3)
   rather than being dropped — **and that entry is still written**, because the error is returned
   after the write, not instead of it. (The workspace has hit ENOSPC mid-run before; this path must
   not go quiet when it happens again.)
6. **The tolerant-reader invariant is stronger than before, not weaker.** The append is now
   genuinely one `write(2)`, so at most one partial final line is possible — R-04-032,
   `store.rs:52-53`, which becomes an accurate comment.
7. **No tokio worker is occupied by file I/O.** The syncer is one OS thread; `append_line` uses no
   `spawn_blocking`; the only `spawn_blocking` is the single teardown barrier (§3.5).
8. **A stale fd is impossible after a `rewrite`.** `rewrite`'s rename is followed by an append that
   lands in the file the path names — the §3.4 invalidation. Note this failure returns `Ok(())`
   when it is wrong, so it must be checked by **reading the file back**, not by checking a result.
9. **The suite is green under the real gate:**
   `cargo test --workspace --features test-fixtures --no-fail-fast`, and
   `cargo clippy --workspace --all-targets --features test-fixtures` exits **0** (the no-panic
   lints do **not** fire under `cargo build`/`cargo test`; check the exit code, not the output).
   Zero *warnings* too, not just zero errors — `type_complexity` (§3.1) is the one this change can
   realistically trip.
