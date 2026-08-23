---
title: Keyedlocks Doc Promises A Clone That Does Not Exist
priority: LOW
stage: done
status: done
updated: 2026-08-23 (closed out)
---

# `KeyedLocks` still has no `Clone`, and its doc still promises one

**QA verdict: nothing landed.** `crates/cyrup-core/src/keyed_lock.rs` on the merged main is 202
lines and contains no `impl Clone for KeyedLocks` and no `#[derive(Clone)]` on the struct. The
type doc at **lines 116–119** is byte-for-byte the text the task quoted as the defect:

```rust
/// A handle onto a caller-owned map of per-key async mutexes.
///
/// Cloning a handle, or constructing a second one over the same map, does NOT create a second lock
/// domain — every handle over one map contends on that map.
pub struct KeyedLocks<K: Eq + Hash + Clone> {
    map: KeyedLockMap<K>,
}
```

`grep -rn "Clone for KeyedLocks" crates/` → 0 hits workspace-wide. The only `Clone` impl in the
file is `impl Clone for KeyedLockMap` (line 93). So the first clause of the type doc still names an
operation the API does not have. Redo the task.

## The file moved under the old spec — three of its instructions are now WRONG

The spec was written against a 126-line `keyed_lock.rs` in which `KeyedLockMap` was a
`type KeyedLockMap<K> = Arc<DashMap<K, Arc<Mutex<()>>>>` alias. That alias is gone: main now has

```rust
pub struct KeyedLockMap<K: Eq + Hash + Clone>(Arc<DashMap<K, Arc<Mutex<()>>>>);
```

(line 34), an opaque newtype with `new`/`contains_key`/`mutex_for`/`ptr_eq` public and
`get_or_insert`/`evict_if_unreferenced` private. Consequences — do **not** paste the old spec's
replacement text verbatim:

1. **`Arc::clone(&self.map)` does not compile.** `self.map` is a `KeyedLockMap<K>`, not an
   `Arc<_>`. The body must be `self.map.clone()`, which routes through the hand-written
   `impl Clone for KeyedLockMap` at keyed_lock.rs:93–99 (`Self(Arc::clone(&self.0))`) — still
   exactly one `Arc` bump, so the "one `Arc` bump" claim in the prose stays true.
2. **The old impl-doc sentence "a derived one would build a fresh `DashMap`" would be a NEW FALSE
   CLAIM** — the exact class of thing this queue exists to remove. `KeyedLockMap` has no `Default`
   impl anywhere (verified: `grep -rn "Default for KeyedLockMap" crates/` → 0; it is refused on
   purpose, keyed_lock.rs:43–48). So `#[derive(Default)]` on `KeyedLocks` is a **compile error**,
   not a silent fresh map. Reword before writing it.
3. **The old `(see [`KeyedGuard`] and `PendingEntry`)` pointer is stale in spirit.** The
   `strong_count == 1` predicate now lives in one place only —
   `KeyedLockMap::evict_if_unreferenced` (line 88–90) — and both drop impls delegate to it. Point
   at that function.

Every line number, the `126 → 144` line count, and the "three pre-existing rustfmt hunks at 71,
112, 139" in the old spec are stale. See settled item S6.

## The change to make

Match on text, not line numbers. This old text occurs exactly once (verified):

```rust
/// domain — every handle over one map contends on that map.
pub struct KeyedLocks<K: Eq + Hash + Clone> {
    map: KeyedLockMap<K>,
}

impl<K: Eq + Hash + Clone> KeyedLocks<K> {
```

Replace with (this exact text was trialled against a copy of main's file in
`/home/user/cyrup/tmp/`: rustfmt-clean, max line length 100, file lands at **223 lines**):

```rust
/// domain — every handle over one map contends on that map. Prefer the clone when a second owner
/// or a spawned task needs a handle: it is one `Arc` bump and, unlike [`KeyedLocks::new`], it
/// cannot be pointed at a different map.
pub struct KeyedLocks<K: Eq + Hash + Clone> {
    map: KeyedLockMap<K>,
}

impl<K: Eq + Hash + Clone> Clone for KeyedLocks<K> {
    /// Hand-written rather than derived so the reason cloning is safe lives on the impl: a clone
    /// is one `Arc` bump on the caller's map (it delegates to [`KeyedLockMap`]'s own hand-written
    /// `Clone`), so it is by construction the SAME lock domain — the only handle-producing
    /// operation that cannot name a different map. It is also invisible to eviction, which gates
    /// `remove_if` on the strong count of the per-key `Arc<Mutex<()>>` *value* (see
    /// `KeyedLockMap::evict_if_unreferenced`), never on references to the map itself.
    ///
    /// There is deliberately no `Default` beside it, and none is derivable: [`KeyedLockMap`] has
    /// no `Default` on purpose, so the only `Default` that could exist here would be a
    /// hand-written one minting a fresh map — the isolated domain this module exists to make
    /// unobtainable.
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}

impl<K: Eq + Hash + Clone> KeyedLocks<K> {
```

Notes on the text above, all verified:

- **The exploded struct literal is mandatory.** `Self { map: self.map.clone() }` on one line is
  NOT rustfmt-clean — trialled, rustfmt emits a hunk exploding it (`struct_lit_width` is 18, the
  field is 21 chars). Written exploded, `rustfmt --check --edition 2024` reports zero hunks.
- **Intra-doc links are `deny`.** `Cargo.toml` `[workspace.lints.rustdoc]` sets
  `broken_intra_doc_links = "deny"`. `[`KeyedLocks::new`]` and `[`KeyedLockMap`]` are safe (both
  public, both already linked elsewhere in this file). `KeyedLockMap::evict_if_unreferenced` is
  deliberately left as a plain code span, not a link, so no resolution risk is introduced. If you
  change any link, gate it with `cargo doc -p cyrup-core`.
- Do **not** add `Default`, `Copy`, a `map()` accessor, tests, doctests, or benchmarks. Do not run
  `cargo fmt`. Do not touch any other file.

## Definition of done (rewritten against main's actual file)

Run from `/home/user/cyrup`.

1. `grep -c 'impl<K: Eq + Hash + Clone> Clone for KeyedLocks<K> {' crates/cyrup-core/src/keyed_lock.rs` → `1`
2. `grep -c 'cannot be pointed at a different map' crates/cyrup-core/src/keyed_lock.rs` → `1`
   and `grep -c 'constructing a second one over the same map' crates/cyrup-core/src/keyed_lock.rs` → `1`
   (both halves of the type doc survive; both are now true of the shipped API).
3. `grep -c 'Arc::clone(&self.map)' crates/cyrup-core/src/keyed_lock.rs` → `0` (it would not compile).
4. `grep -c 'would build a fresh `DashMap`' crates/cyrup-core/src/keyed_lock.rs` → `0` (false claim).
5. Placement: these three greps print strictly increasing line numbers with the `Clone` impl in the
   middle —
   `grep -n 'pub struct KeyedLocks<K: Eq + Hash + Clone> {'`,
   `grep -n 'impl<K: Eq + Hash + Clone> Clone for KeyedLocks<K> {'`,
   `grep -n 'impl<K: Eq + Hash + Clone> KeyedLocks<K> {'` on that file.
6. `wc -l crates/cyrup-core/src/keyed_lock.rs` → `223` (up from 202).
7. `cargo check -p cyrup-core` succeeds. No bound beyond the struct's own `K: Eq + Hash + Clone`
   is needed.
8. `cargo clippy -p cyrup-core --all-targets 2>&1 | grep -c 'keyed_lock.rs'` → `0`.
9. **Zero** rustfmt hunks, not three:
   `rustfmt --check --color never --edition 2024 --config skip_children=true crates/cyrup-core/src/keyed_lock.rs`
   exits `0` with no output. `--edition 2024` is mandatory (without it rustfmt cannot parse
   `async fn` and never runs).
10. No `Default`, `Copy`, or accessor: all three print `0` —
    `grep -c 'Default for KeyedLocks'`, `grep -c 'Copy for KeyedLocks'`, `grep -c 'fn map('`.
11. Only `keyed_lock.rs` differs. `crates/cyrup-core/src/lib.rs`, `crates/cyrup-tools/src/lock.rs`
    and `crates/cyrup-config/src/lock.rs` must be byte-identical (md5sum before and after).

## Settled by QA — do not re-litigate

- **S1. The defect is real and unfixed.** Verified by reading main's `keyed_lock.rs` directly.
- **S2. "Add the impl, keep the sentence" is the right resolution.** The four reasons in the
  original spec hold. `KeyedLocks` is `pub` and re-exported from the crate root
  (`crates/cyrup-core/src/lib.rs:33`), it is a one-field handle, and `KeyedLocks::new(map)` — the
  only handle-producing operation today — *can* be pointed at a fresh isolated map, which
  `clone()` cannot. Adding `Clone` strictly narrows the ways to reach the wrong domain.
- **S3. `Clone` cannot regress eviction.** Verified against the current source: the sole removal
  path is `KeyedLockMap::evict_if_unreferenced` (keyed_lock.rs:88–90),
  `remove_if(key, |_, v| Arc::strong_count(v) == 1)` on the per-key **value** `Arc<Mutex<()>>`.
  `KeyedGuard::drop` (168–176) and `PendingEntry::drop` (195–201) both delegate to it and nothing
  else. A handle clone bumps the *map*'s `Arc` (`KeyedLockMap.0`), which that predicate never
  counts.
- **S4. `cyrup-tools` keeps its `map` field.** Twelve live read sites confirmed in
  `crates/cyrup-tools/src/lock.rs` at lines 252, 253, 255, 256, 277, 280, 301, 309, 314, 326, 341,
  354 — `a.map.ptr_eq(&b.map)`, `b.map.mutex_for(&key)`, `locks.map.contains_key(&key)` across the
  four tests. `Clone` on `KeyedLocks` yields another `KeyedLocks`, not a `KeyedLockMap`, so it does
  not make the field removable. `MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md` stays
  unaffected; do not edit it.
- **S5. `cyrup-config`'s `CONFIG_LOCK_HANDLE` stays.** `crates/cyrup-config/src/lock.rs:22–23`;
  used only by reference in `FileLock::acquire`. `Clone` neither enables nor obviates it.
  Likewise `FileMutationLocks::new`'s two `Arc` clones (tools lock.rs:120–125) stay — the second
  feeds the struct's own `map` field, which a cloned handle cannot supply.
- **S6. The file is already rustfmt-clean.** `rustfmt --check --edition 2024` on main's
  `keyed_lock.rs` exits `0` with no output. The three pre-existing hunks the old spec budgeted for
  no longer exist, so the DoD is "zero hunks", not "three".
- **S7. Baseline compiles.** `cargo check -p cyrup-core` succeeds on main as-is.
