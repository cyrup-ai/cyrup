---
title: Keyedlocks Doc Promises A Clone That Does Not Exist
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:41
---

# Give `KeyedLocks` the `Clone` its own doc already promises

## The defect

[`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs):36-42 — the
type-level doc of the newly extracted primitive opens on cloning, and there is no `Clone` impl
anywhere in the 126-line file:

```rust
/// A handle onto a caller-owned map of per-key async mutexes.
///
/// Cloning a handle, or constructing a second one over the same map, does NOT create a second lock
/// domain — every handle over one map contends on that map.
pub struct KeyedLocks<K: Eq + Hash + Clone> {
    map: KeyedLockMap<K>,
}
```

`locks.clone()` is `error[E0599]: no method named 'clone' found`. The second half of the sentence
("constructing a second one over the same map") is true and is pinned by a test one layer up
(`independent_handles_share_one_lock_per_path`,
[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs):196-227). The first half
is pinned by nothing, because there is nothing to pin.

Newly introduced by this branch: the code this was lifted from documented only construction
(`git show 4902cddf:crates/cyrup-tools/src/lock.rs`, lines 54-58 — "Constructing a second
`FileMutationLocks` does NOT create a second lock domain") and made no claim about cloning.

## Required path: add the impl, keep the sentence

Do **not** delete the clause. `KeyedLocks` is `pub` and re-exported from the crate root
([`crates/cyrup-core/src/lib.rs`](../../crates/cyrup-core/src/lib.rs):33), i.e. it is workspace
substrate, not a private helper, and it is a one-field `Arc` wrapper. Four reasons the impl is the
right side of the choice:

1. **Cloning is the *safest* way to produce a handle, so withholding it is backwards.** The public
   constructor `KeyedLocks::new(map)` accepts any map, including a fresh isolated one — precisely
   the failure the module docs (`keyed_lock.rs`:3-6) call out as the thing that "must not be able
   to accidentally" happen. `clone()` cannot name a different map. Adding it strictly narrows the
   ways to end up in the wrong domain.
2. **It costs one `Arc` bump and cannot regress eviction.** `KeyedGuard::drop` (`:89-99`) and
   `PendingEntry::drop` (`:118-125`) gate `remove_if` on `Arc::strong_count(v) == 1` where `v` is
   the per-key `Arc<Mutex<()>>` **value**. A handle clone touches the *map* `Arc`, which that
   predicate never counts. Verified by reading both drop impls; the added test below pins it.
3. **It matches the crate's own convention for Arc-backed handles.** `CancelToken` (a re-exported
   `tokio_util::sync::CancellationToken`) and `RunCancel`
   ([`crates/cyrup-core/src/cancel.rs`](../../crates/cyrup-core/src/cancel.rs):13) are both
   `Clone`, and the whole cancellation model is "thread it by clone".
4. **The workaround already exists in the tree, one level up.** `cyrup-tools` tests reach for
   `Arc::new(FileMutationLocks::new())` and then `locks.clone()` to hand a domain to a spawned task
   (`crates/cyrup-tools/src/lock.rs`:168/:176 and :391) — an extra `Arc` around a type that is
   already nothing but an `Arc`. Callers who want the same at the `KeyedLocks` layer have no
   cheaper option today.

## Change 1 — the impl

`crates/cyrup-core/src/keyed_lock.rs`, inserted immediately after the struct (i.e. after the
current line 42, before `impl<K: Eq + Hash + Clone> KeyedLocks<K>` at line 44). That placement
mirrors the sibling file's `impl Default for FileMutationLocks`
(`crates/cyrup-tools/src/lock.rs`:72), which also sits between the struct and its inherent impl.

```rust
impl<K: Eq + Hash + Clone> Clone for KeyedLocks<K> {
    /// Hand-written rather than derived so the reason cloning is safe lives on the impl: a clone
    /// is an `Arc` bump on the caller's map, so it is by construction the SAME lock domain — the
    /// only handle-producing operation that cannot name a different map. It is also invisible to
    /// eviction, which gates `remove_if` on the strong count of the per-key `Arc<Mutex<()>>`
    /// *value* (see [`KeyedGuard`] and `PendingEntry`), never on references to the map itself.
    ///
    /// There is deliberately no `Default` beside it: a derived one would build a fresh `DashMap`
    /// and hand back exactly the isolated domain this module exists to make unobtainable.
    fn clone(&self) -> Self {
        Self { map: Arc::clone(&self.map) }
    }
}
```

Notes for the implementer:

- `#[derive(Clone)]` would also compile (the struct already requires `K: Clone`), but the
  *why-it-is-safe* comment is the deliverable here as much as the impl. Same argument the branch
  already made for hand-writing `FileMutationLocks::default`.
- No `#[must_use]` is needed: `clippy::return_self_not_must_use` (workspace lint, `warn`) does not
  fire inside trait impls.
- Do **not** also add `Default`, `Copy`, or a `map()` accessor. `Default` reintroduces the isolated
  domain; an accessor is a separate decision that the sibling task on the `map` field depends on
  *not* being taken (see below).

## Change 2 — the doc sentence

Keep both halves and say what the clone is, so the sentence documents a mechanism instead of just
asserting a property. Replace `keyed_lock.rs`:36-39 with:

```rust
/// A handle onto a caller-owned map of per-key async mutexes.
///
/// Cloning a handle, or constructing a second one over the same map, does NOT create a second lock
/// domain — every handle over one map contends on that map. Prefer the clone when a second owner
/// or a spawned task needs a handle: it is one `Arc` bump and, unlike [`KeyedLocks::new`], it
/// cannot be pointed at a different map.
```

## Change 3 — the test that pins the claim

`keyed_lock.rs` has no test module today; the mechanics are covered from `cyrup-tools`. Add a
minimal one at the end of the file, in the crate's established shape
(`#[cfg(test)]` + `#[allow(clippy::expect_used, clippy::unwrap_used)]`, as in
[`crates/cyrup-core/src/event_stream.rs`](../../crates/cyrup-core/src/event_stream.rs):147-149 —
`expect_used`/`unwrap_used` are `deny` at workspace level, `Cargo.toml`:97-99).

A doctest is **not** an acceptable substitute: `.config/nextest.toml`:15 records that nextest does
not run doctests, so a doctest would leave the claim unenforced by the runner — the exact situation
this task exists to end.

```rust
#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The half of the type doc that had no impl behind it. `cyrup-tools`'
    /// `independent_handles_share_one_lock_per_path` pins the other half (two handles built over
    /// one map) one layer up; nothing pinned this one, because there was nothing to clone.
    #[tokio::test]
    async fn a_cloned_handle_is_the_same_lock_domain() {
        let map: KeyedLockMap<u32> = Arc::new(DashMap::new());
        let a = KeyedLocks::new(Arc::clone(&map));
        let b = a.clone();
        let cancel = CancelToken::new();

        // Same map object behind both handles...
        assert!(Arc::ptr_eq(&a.map, &b.map));

        let ga = a.guard(7, &cancel).await.expect("uncontended");

        // ...and therefore the same mutex for the same key: the clone genuinely cannot enter.
        let via_b = b.map.get(&7).map(|e| Arc::clone(e.value())).expect("entry is present");
        assert!(via_b.try_lock().is_err());

        // Drop order matters: eviction is gated on `strong_count == 1`, so the test's own clone of
        // the value must go BEFORE the guard or the entry is legitimately kept.
        drop(via_b);
        drop(ga);
        assert!(!map.contains_key(&7), "a cloned handle must not keep entries alive");
    }
}
```

Verified against the code rather than assumed:

- `use super::*` is sufficient — `DashMap` (`:13`), `Arc` (`:15`) and `CancelToken` (`:12`) are all
  already imported by the module.
- `a.map` / `b.map` are reachable: a child `mod tests` sees the parent module's private fields.
  This is why the test belongs in `cyrup-core` and needs **no** new accessor.
- The `dashmap::Ref` returned by `get` is consumed inside `.map(..)` and dropped before
  `remove_if` ever runs, so the shard lock is free — same shape as
  `crates/cyrup-tools/src/lock.rs`:218-219.
- `#[tokio::test]` works here: cyrup-core's dev-dependency is workspace `tokio`, which carries
  `macros` and `rt-multi-thread` (`Cargo.toml`:126).

## What must NOT change, and the effect on the sibling task

**`cyrup-tools` keeps its `map` field. `Clone` does not make it removable.** The field
(`crates/cyrup-tools/src/lock.rs`:64-65) exists so the tests can reach the *map*:
`Arc::ptr_eq(&a.map, &b.map)` (:215-216), `b.map.get(&key)` (:218-219),
`locks.map.contains_key(&key)` (:240, :264, :277, :289, :304, :317). `KeyedLocks` still keeps its
map private with no accessor, and `a.clone()` yields another `KeyedLocks`, not a `KeyedLockMap`.
So [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md)
is **unaffected**: its prescription (rewrite the field comment; keep the field) stands verbatim,
and the two tasks can land in either order. Do not edit that file.

Deliberately unchanged, with the reason:

- `FileMutationLocks::new` (`:90-92`) keeps its two `Arc::clone`s. They are two clones because the
  struct has two `Arc`-holding fields, not because handles cannot be cloned — a cloned handle
  cannot supply the `map` field.
- `cyrup-config`'s `static CONFIG_LOCK_HANDLE: LazyLock<KeyedLocks<PathBuf>>`
  ([`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs):21-23) stays. It is
  used **by reference** (`CONFIG_LOCK_HANDLE.guard(..)`, :60), which `Clone` neither enables nor
  obviates; building the handle once in a `LazyLock` is the right shape for a process-wide domain.

Both of those were described as `Clone` workarounds in the original finding; on reading the code
they are not. The case for `Clone` rests on the four reasons above, not on removing them.

## Definition of done

1. `crates/cyrup-core/src/keyed_lock.rs` contains `impl<K: Eq + Hash + Clone> Clone for
   KeyedLocks<K>`, placed between the struct and the inherent impl, with the rationale doc comment
   from Change 1.
2. The type doc retains the cloning sentence, extended as in Change 2 — every clause in it is now
   true of the shipped API.
3. `mod tests` with `a_cloned_handle_is_the_same_lock_domain` is present and passing:
   `cargo test -p cyrup-core keyed_lock` (or `cargo nextest run -p cyrup-core -E 'test(keyed_lock)'`).
4. `cargo clippy -p cyrup-core --all-targets` is clean — in particular no `expect_used`/`panic`
   denials escaping the test module's `allow`.
5. No `Default`, no `Copy`, no map accessor on `KeyedLocks`.
6. Zero lines changed outside `crates/cyrup-core/src/keyed_lock.rs`. `git diff --stat` names that
   one file.
7. Formatting: match the surrounding hand style (one-line `Self { map: Arc::clone(&self.map) }`,
   like `Self { map }` at `:47` and the `PendingEntry { .. }` literal at `:56`). This file is not
   default-rustfmt-clean today — `rustfmt --check` reports three pre-existing diffs at `:53`, `:94`
   and `:121` — and normalising that belongs to
   [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md).
   Do **not** run `cargo fmt` from this task.
