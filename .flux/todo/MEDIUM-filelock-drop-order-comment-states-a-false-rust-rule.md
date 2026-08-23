---
title: Filelock Drop Order Comment States A False Rust Rule
priority: MEDIUM
stage: aug
status: done
updated: 2026-08-23 03:36
---

# `FileLock` field comment justifies the layout with a Rust drop rule that does not exist

## Problem

[`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs) lines 42-49 declare:

```rust
pub struct FileLock {
    /// Layer 1. Declared FIRST so reverse-declaration drop order releases the `flock` BEFORE this —
    /// otherwise a same-process successor wakes out of layer 1 into a still-held `flock` and pays a
    /// pointless trip to the kernel.
    _in_process: KeyedGuard<PathBuf>,
    /// Layer 2, released by [`Drop`] below.
    file: File,
}
```

There is no reverse-declaration drop order for struct **fields**. `Drop::drop` runs first, then
fields drop in **declaration** order. Reverse order is a rule about **locals in a block**.

The outcome is nonetheless correct today, but by a different mechanism than the one documented, and
the field ordering the comment presents as load-bearing in fact produces the opposite of what the
comment claims. This is a comment-accuracy defect on the most concurrency-critical eight lines in
the crate; the correction is the deliverable.

## Research

### 1. The real drop sequence (verified by compiling it)

```rust
struct Field(&'static str);
impl Drop for Field { fn drop(&mut self) { println!("  field drop: {}", self.0); } }
struct S { first: Field, second: Field }
impl Drop for S { fn drop(&mut self) { println!("  S::drop body"); } }
```

Output (rustc, `-O`):

```
STRUCT FIELDS:
  S::drop body            <- Drop::drop body runs FIRST
  field drop: first-declared
  field drop: second-declared   <- fields then drop in DECLARATION order
LOCALS:
  field drop: local-second-declared
  field drop: local-first-declared   <- locals drop in REVERSE order
```

So a `FileLock` drop is:

1. `FileLock::drop` body → `FileExt::unlock(&self.file)` (line 93) → **`flock` released**.
2. `_in_process: KeyedGuard<PathBuf>` drops → **layer 1 released**, successor admitted.
3. `file: File` drops → fd closed. Locking no-op; already unlocked at step 1.

The desired ordering (`flock` gone before layer 1 opens) comes **entirely from the explicit
`unlock` on line 93**. Had the fd close been the thing releasing the `flock`, as the comment
supposes, declaring `_in_process` first would open layer 1 *before* the `flock` released — exactly
the pathology the comment claims the layout prevents.

### 2. The explicit `unlock` predates the branch

`git show 4902cddf:crates/cyrup-config/src/lock.rs` — before the two-layer split the struct was
just `{ file: File }` and `Drop::drop` was already `let _ = FileExt::unlock(&self.file);`. The
branch added `_in_process` and retro-fitted a false justification onto a line that was there for a
different reason. Nothing about the `unlock` was designed to sequence the layers; it happens to.

### 3. `fs4` is `flock(2)`, so the field order is genuinely free

`fs4 1.1.0` (`crates/cyrup-config/Cargo.toml:23`), `src/unix.rs:14` → `FileExt::lock` is
`rustix::fs::flock(fd, LockExclusive)`; `:29` → `unlock` is `flock(fd, Unlock)`. On Windows,
`src/windows.rs` → `LockFileEx` / `UnlockFile` on the raw handle.

`flock` locks live on the **open file description**, not on the process — unlike `fcntl` record
locks, where closing *any* fd on the file drops *all* the process's locks on it. That is what makes
step 2-before-step 3 safe: during the window where layer 1 is open and this `FileLock`'s fd is still
open-but-unlocked, a same-process successor opens its **own** fd on the sidecar and its `flock` is
untouched by our subsequent close. Had `fs4` used `fcntl`, field order would have been load-bearing
after all, and the fix would have to be a reorder rather than a comment.

### 4. The codebase otherwise gets this rule right — do not overcorrect

Two neighbouring sites invoke reverse-declaration order **correctly**, both about locals:

- [`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) lines 110-112
  (`PendingEntry`) and its use at line 55-56: `_pending` is a **local** declared before the `lock`
  local, so it does drop last. Accurate. **Do not touch.**
- [`crates/cyrup-tools/src/ops/local/guard.rs`](../../crates/cyrup-tools/src/ops/local/guard.rs)
  line 30 and [`proc.rs`](../../crates/cyrup-tools/src/ops/local/proc.rs) lines 133-136:
  `kill_guard` is a **local** declared after `child`. Accurate. **Do not touch.**

The false claim is isolated to the one field comment. The corrected text must not read as a blanket
denial of the reverse-order rule.

## Decision: comment-only; keep the field order as-is

Three candidate paths were considered. **Required path: (A).**

**(A) Fix the comment, leave `_in_process` declared first. — CHOSEN.** The invariant then lives in
exactly one place (the unconditional `unlock`, the first statement of `Drop::drop`), and the comment
names it. `_in_process`-first also mirrors the acquisition order in `FileLock::acquire` (layer 1,
then layer 2), the type doc's "Two layers" narrative, and the initializer on line 68 — so the order
is legible rather than arbitrary, and the comment can say plainly that nothing depends on it.

**(B) Reorder to `file` first, so field order is a backstop. — REJECTED.** It would work (fd close
releases the `flock`, then layer 1 opens), but it buys a second, subtler invariant whose only
purpose is to survive deletion of the first — one a future third field would silently break — and it
puts a drop-order claim back into a comment that a maintainer must re-verify. It also churns
concurrency-critical code for a non-defect and breaks the acquisition-order mirroring.

**(C) Wrap `_in_process` in `Option` and `take()` it in `Drop::drop` after the `unlock`. —
REJECTED.** Buys ordering the `unlock` already guarantees, at the cost of a nullable field with a
permanently-`Some` invariant and a matching `Some(..)` in `acquire`.

## Required change

One edit, in [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs). Replace
lines 43-48 (the two field doc comments) so the struct reads **exactly**:

```rust
pub struct FileLock {
    /// Layer 1. Declared first only to mirror the acquisition order in [`FileLock::acquire`];
    /// nothing depends on the position. The ordering that matters — the `flock` gone BEFORE a
    /// same-process successor is admitted through layer 1, so it never wakes into a lock this
    /// process still holds and pays a pointless trip to the kernel — comes from the explicit
    /// `FileExt::unlock` in [`Drop`] below and from nothing else: `Drop::drop` runs FIRST, then
    /// fields drop in DECLARATION order. Reverse-declaration order is a rule about LOCALS, not
    /// fields; `PendingEntry` in [`cyrup_core::keyed_lock`] is the correct use of it. So do not
    /// remove that `unlock` on the theory that closing `file` is equivalent: `file` closes AFTER
    /// this field, which is exactly backwards.
    _in_process: KeyedGuard<PathBuf>,
    /// Layer 2. Released by the explicit `unlock` in [`Drop`] below; closing this fd is only the
    /// backstop for a process that dies holding it. The window between the two field drops, where
    /// the fd is open but unlocked, is harmless: `fs4` is `flock(2)` here, whose lock lives on the
    /// open file description, so the successor's own fd on the sidecar is unaffected.
    file: File,
}
```

Nothing else in the file changes: field order, `acquire`, `open_and_lock`, and the `Drop` impl are
all untouched. All new lines are ≤ 99 columns, within the file's existing maximum of 102, and
rustfmt does not reflow comments (`wrap_comments` defaults off, and there is no `rustfmt.toml`), so
this is format-stable.

Link notes: `[`cyrup_core::keyed_lock`]` already resolves in this file (line 18 uses it).
`PendingEntry` is deliberately in backticks, not a link — it is private in `cyrup-core` and would
not resolve. `[`Drop`]` matches the existing style on line 47.

## Definition of done

- [ ] `crates/cyrup-config/src/lock.rs` lines 43-48 replaced with the block above, verbatim.
- [ ] No other line of `lock.rs` changed — verify with `git diff -- crates/cyrup-config/src/lock.rs`
      that the diff is comment-only and confined to the struct body.
- [ ] Field declaration order still `_in_process` then `file`; the `FileExt::unlock` on line 93
      still present and still the first statement of `Drop::drop`.
- [ ] `crates/cyrup-core/src/keyed_lock.rs` and `crates/cyrup-tools/src/ops/local/{guard,proc}.rs`
      untouched — their reverse-declaration comments are correct and are about locals.
- [ ] `cargo doc -p cyrup-config --no-deps` emits no intra-doc-link warning for `FileLock`.
- [ ] `rustfmt --check --edition 2024 crates/cyrup-config/src/lock.rs` is clean. Do **not** run a
      workspace-wide `cargo fmt`.
- [ ] No new tests. There is no runtime behaviour to assert: the change is text, and the drop
      sequence it describes is a language guarantee, not a property of this crate.

## Interactions

- [`HIGH-dropped-acquire-future-detaches-blocking-flock-task.md`](./HIGH-dropped-acquire-future-detaches-blocking-flock-task.md)
  rewrites `open_and_lock`/`acquire` in the same file. Its primary fix leaves the struct alone; its
  fallback ("move the `KeyedGuard` into the blocking closure") would change the fields, so if that
  fallback is taken, re-check this comment in the same change. No textual conflict either way —
  the two edits touch disjoint line ranges.
- [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
  also touches this crate; keep the scoped `rustfmt` check above rather than a workspace pass.
