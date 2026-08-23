---
title: Keyed Lock Map Alias Exposes The Raw Dashmap
priority: LOW
stage: done
status: done
updated: 2026-08-23 (closed out)
---

# Outstanding: two mandated doc corrections in `cyrup-tools/src/lock.rs` were not applied

**QA rating: 8/10.** The substance of this task — the `KeyedLockMap` newtype, the closed
invariant, the collapsed eviction predicate, and dropping `dashmap` from both consumer manifests —
is done and verified correct. What is left is the conditional pair of corrections this task's own
"Interaction with the sibling tasks" section required, and one of the two is a **live false factual
claim** in a doc comment about the very type this task introduced.

## Why this is not pedantry

The precondition the original spec set — *"**If that block is already in the file when this change
is made**, apply these two corrections as part of this task"* — is satisfied.
`.flux/todo/MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md` carries
`stage: exec / status: done`, and its longer `map`-field doc block is in the tree right now at
`crates/cyrup-tools/src/lock.rs:58-83`. Both search strings below were confirmed present, 1× each.

This is also the one definition-of-done check that still fails outright (DoD #2).

## Item 1 — `crates/cyrup-tools/src/lock.rs:68-71` states something that is now false

Current text:

```rust
    /// - *Reach through `inner`.* There is no route. [`KeyedLocks`] keeps its map private and
    ///   exposes only `new` and `guard` — no accessor, no `Deref`. Adding one would hand every
    ///   holder of a `&KeyedLocks` the raw `DashMap`, `clear()` and `remove()` included, on a map
    ///   whose one-live-mutex-per-key invariant is the entire safety argument of that module.
```

`KeyedLocks<K>`'s `map` field is `KeyedLockMap<K>` (`crates/cyrup-core/src/keyed_lock.rs:121`), not
`Arc<DashMap<..>>`. An accessor returning `&KeyedLockMap<K>` would hand back **only**
`contains_key`, `mutex_for` and `ptr_eq`. Verified against the real source, not inferred:

- `pub struct KeyedLockMap<K: Eq + Hash + Clone>(Arc<DashMap<K, Arc<Mutex<()>>>>);` — the tuple
  field has no `pub`, so `.0` is unreachable outside `cyrup_core::keyed_lock`.
- `grep -n "impl.*Deref\|fn clear\|fn remove" crates/cyrup-core/src/keyed_lock.rs` returns nothing.
- Compile-verified this session: `KeyedLockMap::<PathBuf>::new().clear()` →
  ``error[E0599]: no method named `clear` found for struct `KeyedLockMap<K>` in the current scope``.

So the sentence asserts a hazard that this task deleted. Apply the replacement the spec already
specified. Search text (assert 1× first):

```rust
///   holder of a `&KeyedLocks` the raw `DashMap`, `clear()` and `remove()` included, on a map
///   whose one-live-mutex-per-key invariant is the entire safety argument of that module.
```

Replace with exactly (preserve the existing 4-space indent before each `///`):

```rust
///   holder of a `&KeyedLocks` the whole lock domain, on a map whose one-live-mutex-per-key
///   invariant is the entire safety argument of that module.
```

## Item 2 — `crates/cyrup-tools/src/lock.rs:74` cites an expression that no longer compiles

Current text names `Arc::ptr_eq(&a.map, &b.map)` as "the check". After this task the check at
`crates/cyrup-tools/src/lock.rs:252-253` is `a.map.ptr_eq(&b.map)` / `a.map.ptr_eq(&c.map)`, and the
cited form cannot type-check — `a.map` is a `KeyedLockMap<PathBuf>`, not an `&Arc<T>`. Search text
(assert 1× first):

```rust
///   `Arc::ptr_eq(&a.map, &b.map)`: the check that a separately constructed
```

Replace with exactly:

```rust
///   `a.map.ptr_eq(&b.map)`: the check that a separately constructed
```

Nothing else in that block changes; the rest of it is still accurate (the field is still read by
the same four tests, still per-instance, and `FILE_MUTATION_LOCKS` is still a vacuous substitute
for the identity assertion).

## Re-check after the fix

Only these, plus a re-run of check 11 below since the edit touches a formatted file:

- `grep -rn "dashmap\|DashMap" crates/cyrup-tools crates/cyrup-config` returns **nothing**. Today it
  returns exactly one line: `crates/cyrup-tools/src/lock.rs:70`.
- `grep -n "Arc::" crates/cyrup-tools/src/lock.rs` shows hits only inside `mod tests`. Today it also
  shows `:74`.
- `rustfmt --edition 2024 --check crates/cyrup-tools/src/lock.rs` stays **silent** (it is silent
  today). Both replacement lines are under 100 columns.
- No source file other than `crates/cyrup-tools/src/lock.rs` is touched.

## Settled — verified correct this session, do NOT redo

Everything below was measured against the tree on 2026-08-23, not taken from the spec's word.

1. **The newtype is in place and the invariant is closed.**
   `crates/cyrup-core/src/keyed_lock.rs:34` is the tuple struct; the `impl` block carries `new`,
   `contains_key`, `mutex_for`, `ptr_eq` (public) and `get_or_insert`, `evict_if_unreferenced`
   (private). Hand-written `Clone` at `:93-99`. `#[allow(clippy::new_without_default)]` present with
   its reason comment.
2. **DoD #13 — the closure is really shut.** Both errors reproduced by compiling a scratch probe
   against `target/debug/deps/libcyrup_core-695e8ffcbcb53801.rlib`, edition 2024:
   `E0599` for `.clear()` and ``E0277: the trait bound `KeyedLockMap<PathBuf>: Default` is not
   satisfied`` for `#[derive(Default)]`. Scratch file deleted.
3. **DoD #3.** `grep -rl "dashmap\|DashMap" crates/cyrup-core/src` → only `keyed_lock.rs`;
   `grep -c "^use dashmap" …` → `1`; `grep -c dashmap …` → `3`.
4. **DoD #4.** `grep -c "self\.0" …` → `6`, at `:57, :65, :71, :76, :89, :97` — all inside the
   `impl KeyedLockMap` block or its `Clone` impl. None in `KeyedLocks`, `KeyedGuard`, `PendingEntry`.
5. **DoD #5.** `grep -c "\.remove_if(\|\.or_insert_with(" …` → `2`, both in the private methods.
6. **DoD #6 (first half).** `grep -c "locks\.map\.contains_key" crates/cyrup-tools/src/lock.rs` → `8`.
7. **DoD #7.** `grep -c "Arc" crates/cyrup-config/src/lock.rs` → `1`, the `CONFIG_LOCK_HANDLE` doc
   at `:21`. Both statics rewritten (`:19`, `:23`); `Arc` and `dashmap` imports gone.
8. **DoD #8.** `grep -n "^dashmap" crates/*/Cargo.toml` lists exactly `cyrup-core:18`,
   `cyrup-ext:36`, `cyrup-provider:65`. Root `Cargo.toml:155` unmoved. Neither consumer manifest
   carries `dashmap`.
9. **DoD #9.** `cargo clippy --locked -p cyrup-core -p cyrup-tools -p cyrup-config --all-targets`
   exits 0 with **zero** warnings attributable to `keyed_lock.rs`, `cyrup-tools/src/lock.rs` or
   `cyrup-config/src/lock.rs`. (Unrelated pre-existing warnings elsewhere in the workspace are out
   of scope.) `--locked` held, so `Cargo.lock` did not need to change.
10. **DoD #10.** `cargo doc --locked -p cyrup-core --no-deps` emits no warning at all; the two
    [`KeyedLocks::guard`] intra-doc links resolve.
11. **DoD #11 / #12.** All three files are rustfmt-clean at `--edition 2024` (**0** `Diff in` each).
    The spec predicted `1` remaining in `cyrup-tools/src/lock.rs` for
    `FileMutationLocks::guard`'s trailing chain; that hunk is now written pre-broken at `:178-182`
    and is clean. **This is an improvement over the spec, not a defect** — but note it for
    [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md),
    whose `cyrup-tools` half is now empty. `awk 'length>100'` on `keyed_lock.rs` prints exactly one
    line, `:188`, the pre-existing `PendingEntry` doc sentence (the spec guessed `:182`; harmless).
12. **Every new doc claim checked against vendored `dashmap 6.2.1`**
    (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/dashmap-6.2.1/src/lib.rs`):
    - `pub fn new()` at `:134` is a plain `fn`, **not** `const fn` — the `new()` doc's central claim
      holds, and `LazyLock::new(KeyedLockMap::new)` in both statics compiles.
    - `insert` `:472`, `remove` `:489`, `clear` `:758`, `alter` `:787`, `entry` `:871` all exist and
      are `pub`, so the type doc's list of what an alias would have leaked is accurate.
    - `evict_if_unreferenced`'s doc — *"`remove_if` runs the predicate while holding the shard
      lock"* — is **true**: `_remove_if` takes `_yield_write_shard(idx)` and only then calls
      `f(k, v.get())`.
13. **Consumer call sites.** `KeyedLockMap` appears only in `keyed_lock.rs`, `cyrup-core/src/lib.rs:33`
    (re-export, unchanged), `cyrup-tools/src/lock.rs` (`:12, :28, :84`) and
    `cyrup-config/src/lock.rs` (`:11, :19`). No out-of-workspace caller, nothing else to migrate.
    `crates/cyrup-tools/src/lock.rs:252-256` uses `ptr_eq`/`mutex_for` correctly; `:190` added the
    `use std::sync::Arc;` the parent module lost.

## Still not in this task

- `impl Clone for KeyedLocks<K>` — [`LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md`](./LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md).
  Not present in the tree; when it lands it must use `map: self.map.clone()`, not `Arc::clone(&self.map)`.
- The remainder of the `map` field doc block — [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md).
  Only the two clauses named above belong to this task.
- `MutationGuard` as a newtype — [`LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md`](./LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md).
  (Already landed at `crates/cyrup-tools/src/lock.rs:114`; that task is QA's problem, not this one's.)
- Tests, benchmarks and standalone documentation of any kind. `cyrup_core::keyed_lock` still has no
  tests of its own; that gap is real and belongs to another team.
