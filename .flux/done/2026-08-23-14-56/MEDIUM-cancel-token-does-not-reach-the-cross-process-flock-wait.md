---
title: Cancel Token Does Not Reach The Cross Process Flock Wait
priority: MEDIUM
stage: done
status: done
updated: 2026-08-23 (closed out)
---

# QA: nothing was applied. All three doc edits are still outstanding.

The previous stage recorded `stage: exec / status: done`, but **not one byte of
`crates/cyrup-config/src/lock.rs` was changed.** All three FIND strings from the old §4 are still in
the file, verbatim, exactly once each, and no replacement text exists anywhere in the workspace:

```
$ rg -F 'instead of waiting out a peer process' crates/cyrup-config/src/lock.rs   -> 1 hit (:105)
$ rg -F 'for the microseconds that attempt runs' crates/cyrup-config/src/lock.rs  -> 1 hit (:43)
$ rg -n 'auth\.rs:|models_store\.rs:'            crates/cyrup-config/src/lock.rs  -> :45, :114
$ rg -F -l 'governs BOTH layers, by two different mechanisms' .                   -> no matches
$ rg -F -l 'not themselves cancel-aware'                      .                   -> no matches
```

So the defect this task exists to fix — a rustdoc block asserting a cancellation ceiling that the
constant's own doc 58 lines above denies in capitals — is **fully present**. Re-run exec.

---

## 1. Line numbers, re-derived 2026-08-23 07:57

The old §2 citation audit and §6 composition section are both **obsolete** and have been deleted:
three sibling `lock.rs` tasks (path-resolution, drop-order comment, join-error mapping) landed after
that audit was written and shifted everything. In particular the two
`map_err(|_| ConfigError::Lock { … })` arms the old §6 promised not to touch **no longer exist** —
they are now `join_failed(target, &join)` returning `ConfigError::LockTaskFailed`
(`crates/cyrup-config/src/lock.rs:148`, `:170`, helper at `:367`). None of that disturbs the three
FIND strings.

Current tree, `crates/cyrup-config/src/lock.rs`:

| Item | Lines |
| --- | --- |
| `NEVER_CANCELLED` doc | `:25-33` |
| `MAX_RETRY` doc / const | `:40-46` / `:47` — **EDIT 1 FIND is `:41-46`** |
| `FileLock` type doc | `:49-80` (not touched) |
| field-order comment | `:82-91` (not touched — already landed) |
| `acquire` doc | `:101-119` — **EDIT 2 FIND is `:103-105`**, **EDIT 3 FIND is `:114-115`** |
| `acquire` body | `:120-179` |
| `guard(...).await` | `:126-129` |
| first `spawn_blocking` join | `:144-145` |
| retry `biased select!` | `:157-164` |
| second `spawn_blocking` join | `:167` |
| `open_and_try_lock` / `try_lock` | `:185` / `:205` |

---

## 2. Settled by QA — do not re-litigate, do not re-verify

Everything below was checked against the real source this pass. The replacement text in §3 is
**true**; apply it as written.

- **`acquire` has exactly four await points**, two cancel-aware and two not:
  `guard(...).await` (`:128`, cancel-aware), `spawn_blocking(open_and_try_lock).await` (`:145`, not),
  the retry `select!` (`:157-164`, cancel-aware), `spawn_blocking(try_lock).await` (`:167`, not).
  So "the only await points in `acquire` that are not themselves cancel-aware are the two
  `spawn_blocking` joins" is **exhaustive and true**.
- **`KeyedLocks::guard` really is a `biased` race against the token**:
  `crates/cyrup-core/src/keyed_lock.rs:134` (fn), `:142` (`biased;`), `:148`
  (`_ = cancel.cancelled() => Err(Cancelled)`). Branch 0. True.
- **Neither blocking job waits on a peer**: `open_and_try_lock` is `ensure_dir` + `open(O_CREAT)` +
  one `FileExt::try_lock` (`LOCK_EX|LOCK_NB`); `try_lock` is that one non-blocking call plus an
  `EINTR` retry. "One bounded blocking job, with no cross-process wait inside it" is true, and
  claiming **no** duration for it is right — the join is not in any `select!` with the cancel arm
  and `spawn_blocking` can queue behind a saturated pool.
- **`AuthStore::modify` does hold the guard across `f(current).await`**: `crates/cyrup-config/src/auth.rs`
  — `pub async fn modify` at `:298`, `FileLock::acquire` at `:316`, `let next = f(current).await?`
  at `:324`. EDIT 1's swap of `auth.rs:316-324` for the name is correct and is the right call.
- **EDIT 3's replacement text is true**: `crates/cyrup-config/src/models_store.rs` — `async fn read`
  at `:324` calls `throw_if_aborted` at `:331`, `self.read_latest(options).await` at `:332`, and
  `throw_if_aborted` again at `:338`; `write` (`:344`) checks once at `:355` before its acquire at
  `:357`; `delete` (`:376`) checks once at `:382` before its acquire at `:384`.
- **EDIT 3 is now load-bearing, not cosmetic.** `models_store.rs:311` in the doc has *rotted since
  the spec was written*: line 311 today is `fn write_all(&self, entries: &OrderedObject)`, not a
  re-check. The comment now points at the wrong item — a false claim, not a stale nit.
- **All three intra-doc links survive**: `KeyedLocks` is imported at `:11`, `ConfigError` at `:14`,
  `MAX_RETRY` is in-module. EDIT 2's replacement introduces no new link target and no import change.
- **Widths are fine**: the longest line in each of the three replacement blocks is 99 characters.

---

## 3. The three edits — apply literally

One file, `crates/cyrup-config/src/lock.rs`. **Doc comments only: no signature change, no
behavioural change, no call site touched, no new item, no new dependency.** Each FIND was
re-confirmed this pass to occur **exactly once**, byte-for-byte including the U+2014 em-dashes.

### EDIT 1 — `MAX_RETRY`'s doc (`:41-46`): drop the unbounded duration

FIND:

```rust
/// notice. It does NOT bound cancellation — the cancel arm shares the `select!` with the sleep, so
/// a `cancel()` preempts whatever is left of the tick; only a cancel arriving while an attempt is
/// already in flight waits at all, for the microseconds that attempt runs. Peer hold times span a
/// sub-millisecond JSON read-modify-write (settings, trust, models_store) to a whole OAuth refresh
/// (`auth.rs:316-324` holds the guard across `f(current).await`), so 50 ms is small against the
/// wait it samples and costs at most 20 non-blocking syscalls per second per waiting acquire.
```

REPLACE WITH:

```rust
/// notice. It does NOT bound cancellation — the cancel arm shares the `select!` with the sleep, so
/// a `cancel()` preempts whatever is left of the tick. The only await points in `acquire` that are
/// not themselves cancel-aware are the two `spawn_blocking` joins, so a cancel arriving while an
/// attempt is already in flight is observed when that attempt returns: one bounded blocking job,
/// with no cross-process wait inside it. No duration is claimed for that window — `spawn_blocking`
/// can queue behind a saturated pool. Peer hold times span a sub-millisecond JSON
/// read-modify-write (settings, trust, models_store) to a whole OAuth refresh (`AuthStore::modify`
/// holds the guard across `f(current).await`), so 50 ms is small against the wait it samples and
/// costs at most 20 non-blocking syscalls per second per waiting acquire.
```

Keep the wording "await points **that are not themselves cancel-aware**". "Await points outside that
`select!`" would be false — `guard`'s await is outside it and *is* cancel-aware.

### EDIT 2 — `acquire`'s doc (`:103-105`): the sentence this task exists to fix

Note the leading four spaces; this block is inside `impl FileLock`.

FIND:

```rust
    /// `cancel` governs BOTH layers: layer 1 through the `biased` cancel arm inside
    /// [`KeyedLocks::guard`], layer 2 between retry ticks — so a cancelled acquire returns
    /// [`ConfigError::Cancelled`] within [`MAX_RETRY`] instead of waiting out a peer process.
```

REPLACE WITH:

```rust
    /// `cancel` governs BOTH layers, by two different mechanisms — the difference is load-bearing:
    ///
    /// * Layer 1 is cancelled *in place*: [`KeyedLocks::guard`] races the token against the mutex
    ///   in a `biased` `select!` and returns having taken nothing.
    /// * Layer 2 is cancelled *between attempts*: the retry sleep shares a `biased` `select!` with
    ///   the same token, so a cancel preempts whatever is left of the tick and the acquire returns
    ///   [`ConfigError::Cancelled`] rather than waiting out a peer process. This is NOT bounded by
    ///   [`MAX_RETRY`] — see that constant's doc for what that window actually is.
    ///
```

The trailing `    ///` line is **required**: without it the following paragraph (`/// Dropping this
future is bounded the same way: …`, currently `:106-110`) is absorbed into the last bullet. That
paragraph and the `runtime with a time driver` sentence stay byte-identical.

### EDIT 3 — `acquire`'s doc (`:114-115`): replace the now-false line pointer with a name

FIND:

```rust
    /// (`models_store.rs:311` re-checks after the read; `write`/`delete` deliberately check only
    /// before, matching pi's placement). Dropping the returned guard releases both layers at once.
```

REPLACE WITH:

```rust
    /// (`FileModelsStore`'s `read` re-checks after `read_latest` returns; its `write` and `delete`
    /// deliberately check only before the acquire, matching pi's placement). Dropping the returned
    /// guard releases both layers at once.
```

---

## 4. Explicitly not in scope

- **Do not change the lock mechanism.** No `FileExt::lock`, no `AcquireTask`, no `JoinHandle::abort`,
  no deadline, no retry ceiling, no new tunable, no change to `FIRST_RETRY`/`MAX_RETRY` values. The
  polling design was reviewed, accepted and landed.
- **Do not race the token against the `spawn_blocking` joins.** A `select!` there drops the
  `JoinHandle`, detaching the job, and buys nothing: the job does no cross-process wait, so that
  window is a scheduling artefact. Describing it honestly is the fix.
- **Do not touch** `FileLock`'s type doc (`:49-80`), the field-order comment (`:82-91`),
  `NEVER_CANCELLED`'s doc (`:25-33`), `join_failed`/`ConfigError::LockTaskFailed` (`:148`, `:170`,
  `:367`), or any call site. `acquire`'s signature stays character-for-character
  `pub async fn acquire(target: &Path, cancel: Option<&CancelToken>) -> Result<Self, ConfigError> {`.
- **Pre-existing >100-character lines are NOT yours.** `lock.rs:58`, `:59`, `:252`, `:259` are each
  101 chars today, all inside `FileLock`'s type doc and `lock_key`'s doc. The old DoD claimed the
  file was already clean at ≤100 chars; it is not. Leave them — just keep your own new lines ≤ 99,
  which the §3 replacements already are.
- **No tests, no benchmarks, no new documentation files, no git commands.**

---

## 5. Definition of done

1. `rg -F 'instead of waiting out a peer process' crates/cyrup-config/src/lock.rs` prints nothing.
2. `rg -F 'for the microseconds that attempt runs' crates/cyrup-config/src/lock.rs` prints nothing.
3. `rg -F --count-matches 'not themselves cancel-aware are the two' crates/cyrup-config/src/lock.rs`
   prints `1`.
4. `rg -F --count-matches 'governs BOTH layers, by two different mechanisms' crates/cyrup-config/src/lock.rs`
   prints `1`, with the two bullets and the explicit "NOT bounded by [`MAX_RETRY`]" clause.
5. `rg -n 'auth\.rs:|models_store\.rs:' crates/cyrup-config/src/lock.rs` returns nothing.
6. `rg -n 'FileExt::lock\(' crates/cyrup-config/src/lock.rs` is empty;
   `rg -n 'AcquireTask|JoinHandle|\.abort\(\)|thread::sleep' crates/cyrup-config/src/lock.rs` is
   empty; `rg -c 'spawn_blocking' crates/cyrup-config/src/lock.rs` prints `2`.
7. The only lines differing from the pre-change file are inside `MAX_RETRY`'s doc comment and
   `acquire`'s doc comment; `crates/cyrup-config/src/lock.rs` is the only file changed in the
   workspace. Net `+10` lines (EDIT 1 `+3`, EDIT 2 `+6`, EDIT 3 `+1`).
8. `cargo doc -p cyrup-config` (or `cargo check -p cyrup-config`) emits no new rustdoc-link warning,
   and `cargo fmt --check -p cyrup-config` reports no diff.
