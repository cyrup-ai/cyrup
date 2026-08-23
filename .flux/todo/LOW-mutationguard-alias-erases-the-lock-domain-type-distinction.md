---
title: Mutationguard Alias Erases The Lock Domain Type Distinction
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:49
---

# `MutationGuard` must go back to being a nominal type: a newtype over `KeyedGuard<PathBuf>`

## Problem

[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs) line 85 currently reads:

```rust
pub type MutationGuard = KeyedGuard<PathBuf>;
```

At merge base (`4902cddf`) the same name was `pub struct MutationGuard { inner, lock, map, key }`
with one `Drop` impl, four private fields, and no inherent methods. The extraction into
[`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) moved those four
fields and that `Drop` into `KeyedGuard<K>` and left an alias behind.

Two properties were lost, both about the *name*, neither about the mechanism:

1. **The lock domain is no longer encoded in the type.** `cyrup-config` instantiates the same
   generic over the same key type for a deliberately separate map:
   [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs) line 19
   (`static CONFIG_LOCKS: LazyLock<KeyedLockMap<PathBuf>>`) and line 46
   (`_in_process: KeyedGuard<PathBuf>`), with a comment at `:16-18` stating that config paths and
   tool-mutated paths *are* different key spaces. `cyrup_tools::lock::MutationGuard` and that field
   are now literally the same Rust type. `fn commit(_: MutationGuard)` would accept a guard proving
   exclusion over the wrong key space, silently: both guards lock, both drop correctly, and neither
   excludes the other.
2. **Inherent impls are E0116 inside `cyrup-tools`.** `impl MutationGuard { … }` is
   "cannot define inherent impl for a type outside of the crate where the type is defined". Nothing
   is blocked outright — a local trait can still be implemented for it — but every future addition
   has to be pushed into `cyrup-core` for all domains, or bolted on via an extension trait.

Nothing about the alias is *wrong* today. What is wrong is that a change advertised as behaviour-
and API-preserving quietly downgraded a nominal type to a structural one on the crate's most
concurrency-critical name, and the downgrade is invisible because no caller spells the type.

## Research

### 1. Nothing outside `lock.rs` names the type — verified

```
$ grep -rn "MutationGuard" --include=*.rs crates/ | grep -v CompletionMutationGuard
crates/cyrup-tools/src/lock.rs:81   (doc comment)
crates/cyrup-tools/src/lock.rs:85   pub type MutationGuard = KeyedGuard<PathBuf>;
crates/cyrup-tools/src/lock.rs:144  ) -> Result<MutationGuard, ToolError> {
```

Both mutators bind it anonymously —
[`tools/write.rs`](../../crates/cyrup-tools/src/tools/write.rs) line 102 and
[`tools/edit.rs`](../../crates/cyrup-tools/src/tools/edit.rs) line 223 both say
`let _guard = self.locks.guard(&abs, &cancel).await?;`.
[`lib.rs`](../../crates/cyrup-tools/src/lib.rs) line 43 re-exports only `FileMutationLocks`; the
guard is reachable solely as `cyrup_tools::lock::MutationGuard` because `pub mod lock;` (line 23) is
unchanged. So the fix below is a zero-call-site change.

### 2. A newtype has literally nothing to forward

`KeyedGuard<K>` ([`keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) lines 79-99) has **no
inherent methods and no trait impls other than `Drop`**; all four fields are private to
`cyrup-core`. Its entire public surface is "exists, then drops". The "forwarding boilerplate" cost
normally charged against a newtype is therefore exactly zero here — there is no method, no
`Deref`, no `AsRef` to write.

That is also why the alias leaks only *identity*, not *capability*: unlike `KeyedLockMap`, a holder
of a `KeyedGuard` cannot reach the map or the mutex and cannot break exclusion. This finding is
what keeps the present issue LOW rather than MEDIUM.

### 3. The newtype restores the merge-base public API exactly

`pub struct MutationGuard(KeyedGuard<PathBuf>)` with a private field is, from outside the crate,
indistinguishable from merge base's `pub struct MutationGuard { … }`:

| Property | `4902cddf` struct | `pub type` alias | Proposed newtype |
| --- | --- | --- | --- |
| Nominal (distinct from every other type) | yes | **no** | yes |
| Constructible outside `cyrup-tools` | no | no (`KeyedGuard`'s fields are private) | no |
| Same type as `cyrup-config`'s guard | no | **yes** | no |
| `impl MutationGuard` legal in `cyrup-tools` | yes | **no (E0116)** | yes |
| `Send`/`Sync`/`Unpin` | from the four fields | same | same (auto traits are structural through a newtype) |
| Drop effect / ordering | `Drop` body | field's `Drop` | field's `Drop` |

`FileMutationLocks::guard`'s signature is byte-identical at merge base and HEAD:
`pub async fn guard(&self, path: &Path, cancel: &CancelToken) -> Result<MutationGuard, ToolError>`.
So the newtype reduces this branch's public-API delta for the item to **zero**, which directly
retires row (c) of
[`LOW-public-api-changes-beyond-the-async-keyword.md`](./LOW-public-api-changes-beyond-the-async-keyword.md).

Drop semantics are unchanged and need no `Drop` impl on the wrapper: a struct with no `Drop` of its
own drops its fields in declaration order, so the single field's `KeyedGuard::drop` runs — same
mutex release, same `remove_if(strong_count == 1)` eviction, same `PendingEntry` interaction.
Adding a `Drop` impl to the wrapper would be strictly worse (it would forbid destructuring inside
`lock.rs` for no gain).

### 4. The obvious newtype emits a `dead_code` warning — this is the one real trap

The field is written (by the constructor) and never read, and `rustc` warns on that **even when the
field's type has a `Drop` impl**. Verified against the pinned toolchain
(`rustc 1.98.0 (88d9e12ae 2026-08-18)`, edition 2024) on a standalone file:

```rust
pub struct Inner(String);
impl Drop for Inner { fn drop(&mut self) { let _ = &self.0; } }

pub struct WMapped(Inner);
pub fn e() -> Result<WMapped, ()> { Ok(Inner(String::new())).map(WMapped) }
```

```
warning: field `0` is never read
  --> nt3.rs:15:20
   |
15 | pub struct WMapped(Inner);
   |            ------- ^^^^^
```

Four suppressions were measured on the same file. Results:

| Form | Warns? | Verdict |
| --- | --- | --- |
| `pub struct W(pub Inner);` | no | rejected — a `pub` field lets any crate forge or unwrap a guard |
| `impl Drop for W { fn drop(&mut self) {} }` | **yes** | rejected — does not even work |
| `pub struct W { _inner: Inner }` (`_`-prefix) | no | works; matches `FileLock::_in_process` idiom, but costs a named struct plus `.map(\|g\| W { _inner: g })` and silences by lint quirk rather than by statement |
| `pub struct W(#[expect(dead_code, reason = "…")] Inner);` | no | **chosen** — explicit, and `#[expect]` self-cancels with `unfulfilled_lint_expectation` the day a reader is added |

`#[expect]` is stable since 1.81; the workspace pins `rust-version = "1.96"`
([`Cargo.toml`](../../Cargo.toml) line 89) and there is one existing use in-tree at
[`crates/cyrup-intercom/src/transport/protocol.rs`](../../crates/cyrup-intercom/src/transport/protocol.rs)
line 171. `dead_code` is warn-only here (`[workspace.lints.clippy]` at
[`Cargo.toml`](../../Cargo.toml) lines 97-102 sets no rustc lints), but the file two fields above
already annotates a deliberately-unread field
(`#[cfg_attr(not(test), allow(dead_code))]` on `FileMutationLocks::map`,
[`lock.rs`](../../crates/cyrup-tools/src/lock.rs) line 62), so leaving a fresh warning would be out
of character for the module.

### 5. The exact text below is rustfmt-canonical — verified

The change was applied to a scratch copy of `lock.rs` and run through
`rustfmt --check --edition 2024` (rustfmt 1.9.0-stable, the pinned toolchain). Two results:

- The single-line struct declaration is stable at **96 columns**; every new doc line is ≤ 100. A
  longer `reason` string pushes rustfmt into a six-line `#[expect(…)]` block, which is why the
  argument lives in the doc comment and `reason` stays terse.
- `guard`'s body must be written in the broken-chain form below. The chain
  `.inner.guard(key, cancel).await.map(MutationGuard).map_err(…)` exceeds `chain_width` (60), so
  rustfmt splits it. Writing it pre-split means this task leaves `guard` fmt-clean instead of
  adding a third violation for
  [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
  to sweep up.

After the change the only rustfmt diff left in the file is the pre-existing `FileMutationLocks::new`
struct-literal hunk, which is that other task's business — not this one's.

### 6. No test changes, and no `Debug` bound is introduced

The eight tests in `mod tests` consume the guard only as `let _g = …`, `let g = …`,
`let held = …`, `drop(g)`, and through `Result` combinators. None of those need `MutationGuard:
Debug`: `Result::unwrap`/`expect` bound `E: Debug`, not `T`; `Result::err()` has no bounds;
`assert!(err.is_err())` has none. `FileMutationLocks::key` still returns `PathBuf`, so the direct
map introspection (`locks.map.contains_key(&key)`) is untouched. The "proven with no test changes
at all" property the field comment at
[`lock.rs`](../../crates/cyrup-tools/src/lock.rs) lines 60-63 claims therefore survives this change.

## Decision

**Required path: (A) newtype in `cyrup-tools`.** Alternatives considered and rejected:

**(B) Keep the alias and write a comment saying the trade was deliberate. — REJECTED.** This is the
option the original review offered as a fallback, and it is the wrong one *here specifically*,
because the fix costs one line of code and one `.map`. A comment is the right answer when a newtype
would impose real forwarding cost; §2 shows there is none. It is also strictly worse than (A) on
the API-delta ledger in §3: a comment leaves the branch shipping a `struct` → `type` public-API
change that nobody asked for, while (A) leaves the item unchanged from merge base.

**(C) Newtype the *key* instead — `struct MutationKey(PathBuf)`, then
`KeyedLockMap<MutationKey>` / `KeyedLocks<MutationKey>` / `KeyedGuard<MutationKey>`. — REJECTED.**
Tempting, because the domain genuinely lives in the key space and this separates the map and the
handle as well as the guard. But the map and the handle are already private
(`FileMutationLocks.map`/`.inner` are private fields; `CONFIG_LOCK_HANDLE` is a private static), so
the extra separation guards nothing that can travel. Against that it costs: `Eq + Hash + Clone`
derives, a `Borrow<PathBuf>` impl or explicit wrapping at every `DashMap` lookup, a changed return
type on `FileMutationLocks::key`, and rewrites in five of the eight tests — breaking the
no-test-changes property in §6. Worse ratio, no additional protection.

**(D) Add a domain marker parameter to `cyrup-core`, e.g. `KeyedGuard<K, D = ()>`. — REJECTED.**
Pushes a type parameter that exists purely for nominality onto every current and future domain, to
solve a problem two crates have. Out of proportion.

## Required change

Two edits, both in
[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs). Nothing else in the
workspace changes — not `lib.rs`, not `cyrup-core`, not `cyrup-config`, not the tests, not any
`Cargo.toml`.

### Edit 1 — replace lines 81-85 (the doc comment and the `pub type`)

```rust
/// RAII guard for a per-file mutation lock — a newtype over
/// [`cyrup_core::keyed_lock::KeyedGuard`] keyed by the resolved path. On drop it releases the
/// mutex and evicts the map entry once no other holder/waiter references it (Pi deletes the queue
/// entry when it drains, file-mutation-queue.ts:57-59), so the lock map cannot grow without bound.
///
/// A newtype and NOT a `pub type` alias, deliberately. `cyrup-config` instantiates the same
/// generic over the same key type for its own, deliberately separate map — `CONFIG_LOCKS` and
/// `FileLock::_in_process` (`cyrup-config/src/lock.rs:19` and `:46`) — so an alias would make a
/// guard proving exclusion over config paths and one proving it over tool-mutated paths literally
/// the same Rust type, and `fn commit(_: MutationGuard)` would accept either. Nothing passes a
/// guard as a value today; the wrapper is what keeps the day one does from type-checking against
/// the wrong domain. It costs nothing: `KeyedGuard` has no public operations to forward, and drop
/// order, drop behaviour and auto-trait membership are exactly the field's. It also re-opens
/// `impl MutationGuard` in this crate, which E0116 forbids on the aliased foreign type.
pub struct MutationGuard(#[expect(dead_code, reason = "held for its Drop")] KeyedGuard<PathBuf>);
```

The field stays private and the wrapper gets **no** `Drop` impl of its own — the field's
`KeyedGuard::drop` is the whole behaviour, and adding an outer `Drop` would only forbid
destructuring inside this module.

### Edit 2 — replace the one-line body of `FileMutationLocks::guard` (line 146)

```rust
        let key = Self::key(path).await?;
        self.inner
            .guard(key, cancel)
            .await
            .map(MutationGuard)
            .map_err(|_| error::aborted())
```

`.map` must precede `.map_err`: the success value needs wrapping, the error mapping is unchanged.
Write it broken across lines exactly as shown — that is rustfmt's output for this chain.

The `use cyrup_core::keyed_lock::{KeyedGuard, KeyedLockMap, KeyedLocks};` import at line 12 is still
correct and needs no edit. `guard`'s own doc comment (lines 133-143) is still accurate and must not
be touched.

## Definition of done

- [ ] `crates/cyrup-tools/src/lock.rs` lines 81-85 replaced with the block from Edit 1, verbatim;
      `MutationGuard` is a `pub struct` with one private field and no `Drop` impl of its own.
- [ ] `FileMutationLocks::guard`'s body is the five-line chain from Edit 2, with
      `.map(MutationGuard)` between `.await` and `.map_err`.
- [ ] `git diff -- crates/cyrup-tools/src/lock.rs` shows exactly those two hunks. In particular
      `mod tests` is untouched, and so are `FILE_MUTATION_LOCKS`, `is_missing_path_error`,
      `FileMutationLocks` (fields, `Default`, `new`, `key`) and every doc comment other than the
      guard's own.
- [ ] `git diff -- crates/cyrup-tools/src/lib.rs crates/cyrup-core crates/cyrup-config` is empty.
      `MutationGuard` is deliberately **not** added to `lib.rs`'s root re-exports: this branch is
      already carrying unintended public-API movement, and `cyrup_tools::lock::MutationGuard`
      already names it.
- [ ] `cargo build -p cyrup-tools` produces no `dead_code` warning for `MutationGuard` and no
      `unfulfilled_lint_expectation`. If the latter fires, someone added a reader for the field —
      delete the `#[expect(…)]` rather than widening it.
- [ ] `rustfmt --check --edition 2024 crates/cyrup-tools/src/lock.rs` reports at most the one
      pre-existing `FileMutationLocks::new` hunk (line ~89) and nothing at the guard or the struct.
      Do **not** run a workspace-wide `cargo fmt`.
- [ ] No new tests. There is no runtime behaviour to assert — the wrapper compiles to a move and
      drops to the identical `KeyedGuard::drop`; the existing eight tests in the file already cover
      that drop path and must continue to pass unmodified.

## Consistency with the `KeyedLockMap` finding

[`LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md`](./LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md)
asks the same question about `pub type KeyedLockMap<K> = Arc<DashMap<K, Arc<Mutex<()>>>>`. The
answers agree, and the shared rule is: **a `pub type` alias is fine when the aliased type carries no
invariant of the aliasing crate, and wrong when the alias is that crate's only expression of one.**

Both aliases fail that rule, but not equally, and the difference should be stated rather than
smoothed over:

- `KeyedLockMap` leaks **capability**. Its target is `DashMap`, whose full mutating surface
  (`insert`, `remove`, `clear`, `alter`) is reachable by every domain owner and can produce two live
  mutexes for one key, voiding exclusion. That is a live hazard in `cyrup-tools` in particular,
  which keeps the raw map as a field next to the code that must never break atomicity. Its fix is
  therefore *more* urgent than this one and additionally removes the `dashmap` dependency from two
  crates.
- `MutationGuard` leaks only **identity**. `KeyedGuard`'s fields are private, so no holder can do
  anything but drop it; the only failure mode is a future signature accepting the wrong domain's
  guard. Strictly latent.

So: newtype both. If only one is done, do `KeyedLockMap` first. The two changes compose without
conflict — after both, `FILE_MUTATION_LOCKS` is built with `KeyedLockMap::new()` and
`MutationGuard` still wraps `KeyedGuard<PathBuf>`; neither edit touches the other's lines.

## Interactions

- [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
  owns the two pre-existing fmt hunks in this file (lines 92 and 146). Edit 2 lands on line 146 and
  resolves that one by writing the canonical form; the `FileMutationLocks::new` hunk stays for that
  task. Whichever runs second should re-check, not re-format the crate.
- [`LOW-public-api-changes-beyond-the-async-keyword.md`](./LOW-public-api-changes-beyond-the-async-keyword.md)
  row (c) lists `MutationGuard` as a breaking type change. After this task that row is retired —
  update its table rather than leaving a stale entry. Note it also calls the item
  `cyrup_tools::MutationGuard`; the correct path is `cyrup_tools::lock::MutationGuard`.
- [`LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md`](./LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md)
  is the same two-domains theme from the keying side. It is independent of this change — that one
  is about *how* `cyrup-config` derives its keys, this one about whether the resulting guards are
  distinguishable — but the doc comment in Edit 1 cites `cyrup-config/src/lock.rs:19`/`:46`, so if
  that file's line numbers move, re-check the citation.
- [`MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md`](./MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md)
  rewrites the comment on `FileLock::_in_process`, one of the two lines Edit 1's doc comment cites
  by number. Text only; no conflict.
