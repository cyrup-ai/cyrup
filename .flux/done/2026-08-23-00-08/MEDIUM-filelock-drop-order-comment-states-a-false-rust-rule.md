---
title: Filelock Drop Order Comment States A False Rust Rule
priority: MEDIUM
stage: qa
status: completed
updated: 2026-08-23 07:51
---

# `FileLock` field comment justifies the layout with a Rust drop rule that does not exist

## Problem

In [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs), the declaration of
`pub struct FileLock` (currently lines 74-81; the type is unique in the file, so find it by name)
reads:

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

> **Citation freshness.** Every file:line below was re-verified against the working tree on
> 2026-08-23 05:46. The previous revision of this spec cited the pre-decomposition line numbers
> (struct at 42-49, `unlock` at 93, initializer at 68); those had drifted by +32 / +103 / +79 and
> are corrected here. Where a pointer can be given by name it is, so it cannot rot again.

### 1. The real drop sequence (verified by compiling it)

```rust
struct Field(&'static str);
impl Drop for Field { fn drop(&mut self) { println!("  field drop: {}", self.0); } }
struct S { first: Field, second: Field }
impl Drop for S { fn drop(&mut self) { println!("  S::drop body"); } }
```

Output (`rustc --edition 2024 -O`, run 2026-08-23):

```
STRUCT FIELDS:
  S::drop body                       <- Drop::drop body runs FIRST
  field drop: first-declared
  field drop: second-declared        <- fields then drop in DECLARATION order
LOCALS:
  field drop: local-second-declared
  field drop: local-first-declared   <- locals drop in REVERSE order
```

So a `FileLock` drop is:

1. `FileLock::drop` body → `FileExt::unlock(&self.file)` (`lock.rs:196`, the **first** statement of
   the body) → **`flock` released**.
2. `_in_process: KeyedGuard<PathBuf>` drops → **layer 1 released**, successor admitted.
3. `file: File` drops → fd closed. Locking no-op; already unlocked at step 1.

The desired ordering (`flock` gone before layer 1 opens) comes **entirely from that explicit
`unlock`**. Had the fd close been the thing releasing the `flock`, as the comment supposes,
declaring `_in_process` first would open layer 1 *before* the `flock` released — exactly the
pathology the comment claims the layout prevents.

### 2. The `unlock` is unconditional and is not a sequencing device

Verifiable on disk, without history: `impl Drop for FileLock` (`lock.rs:194-209`) has exactly one
executable statement, `let _ = FileExt::unlock(&self.file);`, and everything after it is the
comment explaining why the `<path>.lock` sidecar is never unlinked. There is no branch, no `cfg`,
and no ordering machinery — the release is unconditional and happens before either field drops
purely because a `Drop` body always precedes field drops. Nothing in the file makes the field
*order* observable. That is what makes this a comment-only defect rather than a code defect.

### 3. `fs4` is `flock(2)`, so the field order is genuinely free

`fs4 = "1.1.0"` (`crates/cyrup-config/Cargo.toml:23`). In
`~/.cargo/registry/src/*/fs4-1.1.0/src/unix.rs`, the `lock_impl!` macro defines `lock` (`:14-16`)
as `rustix::fs::flock(fd, FlockOperation::LockExclusive)`, `try_lock` (`:24-26`) as
`NonBlockingLockExclusive`, and `unlock` (`:29-31`) as `FlockOperation::Unlock`. On Windows,
`src/windows.rs` uses `LockFileEx` (`:45`, `:64`) / `UnlockFile` (`:33`) on the raw handle.

`flock` locks live on the **open file description**, not on the process — unlike `fcntl` record
locks, where closing *any* fd on the file drops *all* the process's locks on it. That is what makes
step 2-before-step 3 safe: during the window where layer 1 is open and this `FileLock`'s fd is still
open-but-unlocked, a same-process successor opens its **own** fd on the sidecar (via
`open_and_try_lock`, `lock.rs:156`) and its `flock` is untouched by our subsequent close. Had `fs4`
used `fcntl`, field order would have been load-bearing after all, and the fix would have to be a
reorder rather than a comment.

### 4. The codebase otherwise gets this rule right — do not overcorrect

Two neighbouring sites invoke reverse-declaration order **correctly**, both about locals. Both were
re-verified and both citations still hold:

- `PendingEntry` in [`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs)
  — type doc at lines 110-112, and the local it describes at lines 55-56 inside `KeyedLocks::guard`:
  `_pending` is a **local** declared before the `lock` local, so it does drop last. Accurate.
  **Do not touch.**
- `KillTreeOnDrop` in [`crates/cyrup-tools/src/ops/local/guard.rs`](../../crates/cyrup-tools/src/ops/local/guard.rs)
  line 30, and its use in [`proc.rs`](../../crates/cyrup-tools/src/ops/local/proc.rs) lines 133-136:
  `kill_guard` is a **local** declared after `child`. Accurate. **Do not touch.**

The false claim is isolated to the one field comment. The corrected text must not read as a blanket
denial of the reverse-order rule — it names `PendingEntry` as the correct use.

## Decision: comment-only; keep the field order as-is

Three candidate paths were considered. **Required path: (A). (B) and (C) are recorded so they are
not re-proposed; do not implement them.**

**(A) Fix the comment, leave `_in_process` declared first. — REQUIRED.** The invariant then lives in
exactly one place (the unconditional `unlock`, the first statement of `Drop::drop`), and the comment
names it. `_in_process`-first also mirrors the acquisition order in `FileLock::acquire` (layer 1 via
`CONFIG_LOCK_HANDLE.guard`, then layer 2 via `open_and_try_lock`) and the `Ok(Self { .. })` that
ends `acquire` (`lock.rs:146-149`), as well as the type doc's "Two layers" narrative — so the order
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

Exactly one edit, in `crates/cyrup-config/src/lock.rs`. It is a whole-block replacement, so no line
number is needed to apply it.

**Find this exact text.** It occurs **exactly once** in the file (verified: match count = 1; each of
its anchor lines `pub struct FileLock {`, `/// Layer 1. Declared FIRST`, and
`/// Layer 2, released by [`Drop`] below.` is also unique on its own). Note the two `—` are U+2014
EM DASH, not hyphens, and the indentation is four spaces:

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

**Replace it with this exact text** (again, every `—` is U+2014; four-space indentation):

```rust
pub struct FileLock {
    /// Layer 1. Declared first only to mirror the acquisition order in [`FileLock::acquire`] and
    /// the `Ok(Self { .. })` that ends it; nothing depends on the position. The ordering that
    /// matters — the `flock` gone BEFORE a same-process successor is admitted through layer 1, so
    /// it never wakes into a lock this process still holds and pays a pointless trip to the
    /// kernel — comes from the explicit `FileExt::unlock` that is the first statement of the
    /// [`Drop`] impl below, and from nothing else: the `drop` body runs FIRST, then fields drop in
    /// DECLARATION order. Reverse-declaration order is a rule about LOCALS, not fields —
    /// `PendingEntry` in [`cyrup_core::keyed_lock`] is the correct use of it. So do not delete
    /// that `unlock` on the theory that closing `file` is equivalent: `file` closes AFTER this
    /// field, which is exactly backwards.
    _in_process: KeyedGuard<PathBuf>,
    /// Layer 2. Released by the explicit `unlock` in the [`Drop`] impl below; closing this fd is
    /// only the backstop for a process that dies holding it. The window between the two field
    /// drops, where the fd is open but unlocked, is harmless: `fs4` is `flock(2)` here, whose lock
    /// lives on the open file description, so a successor's own fd on the sidecar is unaffected.
    file: File,
}
```

Nothing else in the file changes: field order, `FileLock::acquire`, `open_and_try_lock`, `try_lock`,
and the `Drop` impl are all untouched. (The prior revision of this spec called the helper
`open_and_lock`; there is no such function — the two free helpers are `open_and_try_lock` and
`try_lock`.)

The block grows the file by 10 lines, so after the edit `_in_process` sits at line 85, `file` at
line 90, and `FileExt::unlock` moves from line 196 to line 206. Anything citing those numbers is
listed under Interactions.

### Pre-verified formatting facts

- All 18 replacement lines are ≤ 99 characters (longest: 99). The file's existing maximum is 102,
  so nothing regresses.
- There is no `rustfmt.toml` or `.rustfmt.toml` anywhere in the workspace, so `wrap_comments`
  is at its default `false` and rustfmt will not reflow these lines.
- `rustfmt --check --edition 2024 crates/cyrup-config/src/lock.rs` exits 0 **today**, and exits 0
  on a scratch copy carrying this exact replacement. The edit is format-stable.

### Pre-verified doc-link facts

The replacement introduces **no new intra-doc-link syntax**. All three link forms already appear in
this same file and already resolve:

- `[`FileLock::acquire`]` — already used at `lock.rs:58`.
- `[`Drop`]` — already used at `lock.rs:79`, the very line being replaced, and reintroduced by the
  replacement text.
- `[`cyrup_core::keyed_lock`]` — already used at `lock.rs:19`; the crate is imported at `lock.rs:11`.

`PendingEntry` is deliberately in backticks, **not** a link: it is a private item in `cyrup-core`
and a link would not resolve. `FileExt::unlock` is likewise plain backticks. Do not "improve" either
into a link.

## Definition of done

Verify each by reading the file on disk. No git, no tests.

- [ ] `crates/cyrup-config/src/lock.rs` contains the replacement block above **verbatim**, and a
      search for the string `Declared FIRST so reverse-declaration` returns **zero** hits in the
      whole repository's `crates/` tree.
- [ ] A search for the replacement's opening phrase, `Declared first only to mirror`, returns
      **exactly one** hit, in `crates/cyrup-config/src/lock.rs`.
- [ ] Field declaration order is still `_in_process` then `file`, and the struct still has exactly
      those two fields.
- [ ] `impl Drop for FileLock` still has `let _ = FileExt::unlock(&self.file);` as its **first and
      only** executable statement.
- [ ] The file is 296 lines (it is 286 today; the block adds 10). No other region of the file
      differs — confirm by diffing against a pre-edit copy you took yourself, e.g.
      `cp crates/cyrup-config/src/lock.rs tmp/lock.rs.bak` before editing and
      `diff tmp/lock.rs.bak crates/cyrup-config/src/lock.rs` after; the only hunk must be inside
      the `pub struct FileLock { ... }` body.
- [ ] `crates/cyrup-core/src/keyed_lock.rs`, `crates/cyrup-tools/src/ops/local/guard.rs` and
      `crates/cyrup-tools/src/ops/local/proc.rs` are byte-identical to before — their
      reverse-declaration comments are correct and are about locals.
- [ ] `rustfmt --check --edition 2024 crates/cyrup-config/src/lock.rs` exits 0. Run it on this one
      file only; do **not** run a workspace-wide `cargo fmt`.
- [ ] No test file is added or edited, and no benchmark or external documentation is written.
      There is no runtime behaviour to assert: the change is text, and the drop sequence it
      describes is a language guarantee, not a property of this crate.

## Interactions

- `HIGH-dropped-acquire-future-detaches-blocking-flock-task.md` has **already landed** — it now
  lives in `.flux/done/2026-08-23-00-08/`, and the non-blocking retry loop it specified is present
  in `FileLock::acquire` today. It took its primary fix, not the fallback: the `KeyedGuard` is still
  a struct field, not moved into the blocking closure. **No conflict remains, and no re-check of
  this comment is owed.** (The prior revision of this spec still described that task as pending.)
- [`LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md`](./LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md)
  writes a doc comment that cites `cyrup-config/src/lock.rs:19` and `:46` by number and names
  `FileLock::_in_process`. This edit does not move line 19 or 46 (both precede the struct), so those
  two citations survive. Text only; no conflict.
- Nine other queued tasks name `crates/cyrup-config/src/lock.rs`
  (`LOW-branch-leaves-rustfmt-violations-…`, `LOW-config-lock-skips-the-path-resolution-…`,
  `LOW-keyed-lock-map-alias-…`, `LOW-keyedlocks-doc-promises-a-clone-…`,
  `LOW-models-store-rationale-…`, `LOW-public-api-changes-…`,
  `LOW-rewritten-concurrency-test-doc-…`, `LOW-spawn-blocking-join-error-…`,
  `MEDIUM-cancel-token-does-not-reach-the-cross-process-flock-wait.md`). None edits the struct
  body, but several quote line numbers **at or after** line 74 — chiefly the `Ok(Self { .. })` in
  `acquire` and the `FileExt::unlock` in `Drop`. Whichever of these runs second must re-derive its
  line numbers; the +10 shift from this edit applies to everything below `pub struct FileLock {`.
- [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
  lists this file in its table. Its count for `cyrup-config/src/lock.rs` is already 0 on disk today,
  and this edit keeps it at 0. Keep the scoped `rustfmt` check above rather than a workspace pass.

## QA verdict (2026-08-23 07:51) — PASS, 9/10

Reviewed read-only against the working tree; no source file touched.

Verified:
- The false claim is gone: `Declared FIRST so reverse-declaration` returns **0** hits under
  `crates/`; `Declared first only to mirror` returns exactly **1**, at `lock.rs:82`.
- The 18-line replacement block is present **verbatim** (`lock.rs:81-98`), four-space indent, the
  only non-ASCII characters in it being U+2014 em dashes.
- Field order unchanged (`_in_process` then `file`, two fields only); `impl Drop for FileLock`
  (`lock.rs:223-238`) still has `let _ = FileExt::unlock(&self.file);` as its first and only
  executable statement.
- The drop sequence the new text asserts was re-verified by compiling it (`rustc --edition 2024`):
  `Drop::drop` body first, then fields in **declaration** order; locals in **reverse**.
- `fs4 1.1.0` (Cargo.lock) is `flock(2)` on unix — `src/unix.rs` `lock_impl!` routes
  `lock`/`try_lock`/`unlock` through `rustix::fs::flock`, so the "lock lives on the open file
  description" justification for the harmless open-but-unlocked window is correct.
- The acquisition-order mirroring claim holds: `acquire` takes layer 1 (`CONFIG_LOCK_HANDLE.guard`)
  then layer 2, and ends `Ok(Self { _in_process, file })`.
- `PendingEntry` cross-reference is accurate and untouched: `_pending` is a **local** declared
  before the `lock` local in `KeyedLocks::guard` (`keyed_lock.rs:135-140`). `KillTreeOnDrop`
  (`ops/local/guard.rs:31`, `ops/local/proc.rs:133`) is likewise about locals and untouched.
- `rustfmt --check --edition 2024 crates/cyrup-config/src/lock.rs` exits 0.
- All three intra-doc links resolve: `cargo doc -p cyrup-config` with `--document-private-items`
  renders `[`FileLock::acquire`]`, `[`Drop`]` and `[`cyrup_core::keyed_lock`]` as real anchors in
  `struct.FileLock.html`; `PendingEntry` and `FileExt::unlock` remain plain backticks.

Deviations from the spec that are **not** defects:
- The spec's "file is 286 lines today, 296 after" is stale — the file is 372 lines because other
  landed work (the non-blocking retry loop, `NEVER_CANCELLED`/`MAX_RETRY` docs) grew regions above
  and below the struct. The struct block itself matches the spec byte-for-byte.
- Consequently the spec's `_in_process`/`file`/`unlock` line numbers (85/90/206) are off; actual
  are 92/97/225. Stale numbers in a spec, not false claims in shipped source.

Remaining nits (not worth rework):
- "`fs4` is `flock(2)` here" is the unix path; Windows uses `LockFileEx`/`UnlockFile` on the raw
  handle, which is also per-handle, so the conclusion holds on both platforms.
- "closing this fd is only the backstop for a process that dies holding it" elides the (ignored)
  error return of `FileExt::unlock`. Harmless understatement.
