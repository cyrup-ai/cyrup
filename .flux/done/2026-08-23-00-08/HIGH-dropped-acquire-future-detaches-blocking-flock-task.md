---
stage: qa
status: completed
title: Dropped Acquire Future Detaches Blocking Flock Task
priority: HIGH
updated: 2026-08-23 09:10
---

# QA 4/10 — the doc-correction pass shipped a new false claim

Three of the five new claims are true and were verified against tokio 1.52.3, tokio-util 0.7.18 and
fs4 1.1.0 source. **One is provably false.** On a branch whose purpose is deleting false claims from
comments, that is a blocking defect, and it is worse than what it replaced: the old text was vaguely
wrong, this pass restated it *more* precisely and *more* emphatically.

The lock mechanism is untouched and remains accepted. Do not revisit it.

---

## 1. REQUIRED — `lock.rs:30-32` states something false

```rust
/// … so both `select!`s that consume it —
/// [`KeyedLocks::guard`]'s and the layer-2 retry loop below — always take their other arm. An
/// uncontended acquire never polls it at all (registration happens on first poll, and the retry
/// loop is skipped), so the common path costs one stack-pinned struct and no syscall.
```

The sentence names **both** `select!`s and then reasons about only one. Proof, each link checked:

| step | evidence |
| --- | --- |
| every acquire calls `guard`, contended or not | [`lock.rs:105-108`](../../crates/cyrup-config/src/lock.rs) |
| that `select!` is `biased;` with `_ = cancel.cancelled()` as **branch 0** | [`keyed_lock.rs:62-69`](../../crates/cyrup-core/src/keyed_lock.rs) |
| `biased;` sets `start=0`, so branch 0 is polled **first, every poll** | `tokio-1.52.3/src/macros/select.rs:796-798`, `:671-679` |
| that poll calls `is_cancelled()` and polls the inner `Notified` | `tokio-util-0.7.18/src/sync/cancellation_token.rs:329-346` |
| which pushes a waiter under `notify.waiters.lock()` | `tokio-1.52.3/src/sync/notify.rs:1118-1149` |

So the uncontended path **does** poll `NEVER_CANCELLED` and **does** register (then unregister) a
waiter on that process-global token. The real cost is two `WaitForCancellationFuture`s — guard's,
plus the one pinned at `lock.rs:123-124` — one `TreeNode` mutex round-trip, and one `Notify::waiters`
insert and remove.

The parenthetical is a non-sequitur: "the retry loop is skipped" is true of **this file's** future
only. `"no syscall"` survives (an uncontended `std::sync::Mutex` is atomics-only on Linux) and so
does "no allocation" (the waiter is intrusive, the waker clone a refcount bump) — but
`"never polls it at all"` does not.

**Fix:** say what is actually true. The token is polled exactly once per acquire, inside
`KeyedLocks::guard`'s biased branch 0, and never again on the uncontended path because the retry
loop is skipped. Keep the "no syscall / no allocation" point, which is sound. Do not reintroduce any
form of "never polled".

## 2. REQUIRED — `lock.rs:87` now contradicts `lock.rs:41`

`acquire`'s doc still says a cancelled acquire "returns [`ConfigError::Cancelled`] within
[`MAX_RETRY`]", 46 lines below the new and correct "It does NOT bound cancellation." A reader meets
both. The retired spec blessed `:87` as "a correct over-bound" — but per item 3 it is not a bound at
all in the queued-`spawn_blocking` case; it is an empirical estimate written as a guarantee. If
`MAX_RETRY`'s doc was worth correcting for this, so is this line.

**Fix:** restate `:87` in terms of what actually happens — cancellation preempts the retry sleep, so
it is observed as soon as any in-flight attempt returns — rather than quoting a ceiling that the
same file now says does not apply.

## 3. REQUIRED — `lock.rs:42-43` quantifies a window it cannot bound

"only a cancel arriving while an attempt is already in flight waits at all, for the microseconds
that attempt runs." The *shape* is right — all four await points were walked and only the two
`spawn_blocking` awaits qualify — but the duration is asserted, not bounded, for two reasons:

- The prologue window is not just a `flock` attempt. It is `ensure_dir` (`:224` — a `stat`, possibly
  `create_dir_all` + `set_permissions`), then `open(O_CREAT)` (`:159-165`), *then* the attempt.
- `spawn_blocking` can **queue** behind a saturated blocking pool, and that `JoinHandle` await is
  not in any `select!` with the cancel arm — so the window has no upper bound at all.

**Fix:** keep the exhaustiveness claim, drop the microsecond figure. Say the window is one bounded
blocking job — no `flock` wait in it — without asserting a duration.

## 4. REQUIRED — update this file's own stale references for downstream tasks

The five edits shifted `lock.rs` by +6 lines, invalidating pointers this task hands to others:

| where | says | should say |
| --- | --- | --- |
| Interactions → `LOW-spawn-blocking-join-error…` | the two `map_err` sites are `:111`, `:133` | **`:117`, `:139`** |
| Interactions → `MEDIUM-filelock-drop-order…` | the struct field doc is `:69-71` | **`:74-76`** |
| DoD 3 | `ensure_dir`'s `stat` is at `:218` | **`:224`** |
| item 1 above / former DoD | the retry `select!` is `:121-128` | **`:127-134`** |

## 5. Assigned elsewhere — do NOT fix here

The same +6 shift broke two citations in a sibling file:
[`models_store.rs:201-202`](../../crates/cyrup-config/src/models_store.rs) cites `lock.rs:99-102`
and `:126` for the two `Cancelled` sites, which are now `:108` and `:132`.

This is real and must be fixed, but **not in this task**: DoD 6 requires exactly one file to change,
which makes the two requirements mutually unsatisfiable — a genuine spec defect worth naming. It is
already assigned to
[`MEDIUM-lock-cancellation-reported-as-model-source-not-aborted.md`](./MEDIUM-lock-cancellation-reported-as-model-source-not-aborted.md),
whose own QA marked it required. That file's rework also carries the better suggestion: **drop the
line numbers entirely** — "over the single `Err` arm of `KeyedLocks::guard`" and "the layer-2 retry
loop" already name their targets unambiguously, and a nameless reference cannot rot. Apply the same
judgement to any pointer this task rewrites.

---

## Record only — verified, out of scope

- **`lock.rs:74-76` states a false Rust rule** — "Declared FIRST so reverse-declaration drop order
  releases the `flock` BEFORE this". Struct fields drop in *declaration* order, not reverse. The
  behaviour is right anyway because `Drop for FileLock` (`:193-195`) unlocks before any field drops.
  Owned by `MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md`.
- **`lock.rs:184-185`'s rationale is contradicted 20 lines up.** "`<path>.lock` is an implementation
  detail no operator opens" — but `open_and_try_lock` at `:165` does `map_err(|e| io_err(lock_path,
  e))`, surfacing the sidecar path in `ConfigError::Io`. Placement is correct; the rationale is
  pre-existing and inherited.
- **`startup_ui.rs:872`** cites `cyrup-config/src/lock.rs:13-38` for the never-unlinked sidecar; that
  comment is at `:196-206`. Wrong before this pass too — pre-existing, another crate.
- **`lock.rs:53`** ("no syscall and no polling" for layer 1) carries the same latent inaccuracy as
  item 1, pre-existing.

## Already accepted — do not redo

Verified against vendored source, not taken on trust.

- **Claim: `MAX_RETRY` does not bound cancellation** — TRUE. A `cancel()` during `sleep` wakes the
  select immediately: the pinned future registered on the biased first poll, `cancel()` reaches
  `notify_waiters()` (`tree_node.rs:366`), the select re-polls branch 0 and `is_cancelled()` is
  true. Registration persists across ticks because the future is `&mut`-borrowed, never rebuilt.
- **Claim: at most one bounded attempt in flight, whose fd tokio closes** — TRUE, both halves. Each
  `spawn_blocking` is awaited to completion before the next is spawned. On completion
  `harness.rs:339-344` runs `drop_future_or_output()` when `JOIN_INTEREST` is clear, dropping the
  `Result<(File, bool), _>`, closing the fd, releasing the `flock`. Moving the `File` *through*
  `try_lock` rather than borrowing is what makes this hold on every exit.
- **Claim: requires a time driver; uncontended never reaches the sleep** — TRUE. `driver.rs:115`
  panics with "timers are disabled … call `enable_time`", and `while !held` is not entered when the
  prologue won.
- **Claim: `models_store.rs:311`** — TRUE. That is the post-`read_latest` `throw_if_aborted` in
  `read`; `write` (`:328`) and `delete` (`:355`) check only before. The whole sentence holds.
- **The relocated comment's placement** — correct. `Ok(())`, `WouldBlock` and `Interrupted` build no
  error, so `Err(TryLockError::Error(_))` is the only path-bearing arm in the function.
- **DoD 1, 2, 3, 5, 6 met.** Arm order and semantics intact; `rg 'FileExt::lock\('` empty; nothing
  in a blocking closure waits on another process; "at the same instant" gone; only `lock.rs` moved.
- **Other citations spot-checked correct:** `auth.rs:316-324`, `fs4-1.1.0/src/unix.rs:56-57`, the
  two tokio `blocking.rs` quotes, `keyed_lock.rs:62`. Every new line is ≤100 characters — the
  over-100 byte counts are em-dashes.
