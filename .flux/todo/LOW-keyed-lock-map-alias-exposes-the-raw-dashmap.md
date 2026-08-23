---
title: Keyed Lock Map Alias Exposes The Raw Dashmap
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:45
---

# Newtype `KeyedLockMap`: make the one-mutex-per-key invariant a compile error and drop `dashmap` from both consumers

Three source files and two manifests:
[`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs),
[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs),
[`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs),
[`crates/cyrup-tools/Cargo.toml`](../../crates/cyrup-tools/Cargo.toml),
[`crates/cyrup-config/Cargo.toml`](../../crates/cyrup-config/Cargo.toml). No behaviour change, no
new tests, no new dependency. Measured against a patched copy of each file: `+79 −16` in
`keyed_lock.rs` (most of it doc), `+15 −13` in `cyrup-tools/src/lock.rs`, `+3 −4` in
`cyrup-config/src/lock.rs`, and one deleted line in each manifest.

## The decision: newtype. Not "document the invariant instead"

Asked plainly, and the answer is plainly the newtype — but on evidence, not on principle. The two
arguments that would normally make a newtype not worth it both fail here, and one of them fails
outright.

**The ergonomic objection is false.** The received wisdom was that both consumers declare their own
`static LazyLock<KeyedLockMap<K>>`, so a newtype would need a `const`-constructible `new()` to keep
working in a `static`. It does not. `LazyLock::new` is `const fn` in the sense that it *stores* an
initializer; the initializer runs on first deref, on the heap, at runtime. The current code already
relies on this — `DashMap::new()` is not `const fn` (`dashmap-6.2.1/src/lib.rs:134`, an ordinary
`fn` in `impl<K: Eq + Hash, V> DashMap<K, V, RandomState>`), and `Arc::new` is not either. A plain
`pub fn new() -> Self` is exactly as valid in a `static`, and the call site gets *shorter*:

```rust
// before
static CONFIG_LOCKS: LazyLock<KeyedLockMap<PathBuf>> = LazyLock::new(|| Arc::new(DashMap::new()));
// after
static CONFIG_LOCKS: LazyLock<KeyedLockMap<PathBuf>> = LazyLock::new(KeyedLockMap::new);
```

Verified: a standalone crate mirroring `keyed_lock.rs` with the newtype plus both consumers'
statics and four of the `cyrup-tools` tests compiles warning-free and passes 4/4 under
`dashmap 6.2.1` / `tokio 1.53.1` / rustc 1.98.0, edition 2024. Fn-item-to-fn-pointer coercion
infers `K = PathBuf` from the `LazyLock<KeyedLockMap<PathBuf>>` annotation with no turbofish.

**The compatibility objection does not apply.** `KeyedLockMap` is one commit old. It was a private
field of `FileMutationLocks` at merge base and has exactly two consumers, both in this workspace,
both on this branch. There is no external caller to break and no deprecation to run. This is the
only moment at which the change costs nothing; every later one costs a breaking release.

**What the newtype buys, concretely:**

1. The invariant stops being convention. After the change, `insert`, `remove`, `clear`, `alter` and
   `entry` are unreachable from outside `cyrup_core::keyed_lock` — the two operations that can
   touch the map (`get_or_insert`, `evict_if_unreferenced`) are private methods on the newtype, and
   `self.0` appears nowhere outside its own `impl` block.
2. `dashmap` leaves both consumer manifests. `cyrup-config` took the dependency **on this branch**
   ([`crates/cyrup-config/Cargo.toml:22`](../../crates/cyrup-config/Cargo.toml), added in this
   diff) purely to write `Arc::new(DashMap::new())` once. `cyrup-tools` had it at merge base only
   because it owned the map; after the extraction its three remaining `dashmap` mentions are all in
   `lock.rs` and all serve this one type. Both drop to zero uses, so both drop the dep — and with
   it the version-lockstep coupling with `cyrup-core`.
3. The duplicated eviction predicate collapses. `KeyedGuard::drop` and `PendingEntry::drop` today
   hold byte-identical `remove_if(&self.key, |_, v| Arc::strong_count(v) == 1)` calls, with a
   comment on the second saying "Identical predicate to `KeyedGuard::drop`". One private method now
   holds it once.
4. A live footgun becomes a compile error. Verified both halves: with the alias,
   `#[derive(Default)] struct S { map: KeyedLockMap<PathBuf> }` compiles and mints a **fresh,
   isolated** map — the exact bug `FileMutationLocks::default` is hand-written to avoid
   ([`crates/cyrup-tools/src/lock.rs:72-79`](../../crates/cyrup-tools/src/lock.rs)). With the
   newtype and no `Default` impl, the same derive is `error[E0277]: the trait bound
   `KeyedLockMap<PathBuf>: Default` is not satisfied`.

**What it costs, honestly:** ~55 lines of new code and doc in `keyed_lock.rs`; three public
read-only observers whose only present caller is one `cyrup-tools` test; one
`#[allow(clippy::new_without_default)]`. That is the whole bill.

**When "document instead" would have been right:** if `keyed_lock` were established API with
callers outside this workspace. Then the observers would have to be grown one at a time as each
consumer's need surfaced, the mutating surface would already be depended on, and a doc paragraph
plus the existing `map` field comment would be the proportionate answer. Neither condition holds.

## Research — what was verified

### 1. The finding's own suggested fix does not work as written

Three defects, all of which the required change below corrects:

- **`#[cfg(test)]` does not cross a crate boundary.** The finding proposes a `#[cfg(test)]`-gated
  `KeyedLocks::contains_key` for "the existing test-introspection in `cyrup-tools/src/lock.rs`".
  `cfg(test)` is set per compilation unit; when `cyrup-tools` builds its test target, `cyrup-core`
  is still built as an ordinary dependency with `cfg(test)` **off**, so the method would not exist.
  (Same conclusion the sibling
  [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md)
  reaches at its rejected path 2.)
- **`contains_key` is not enough.** The tests at
  [`crates/cyrup-tools/src/lock.rs:215-219`](../../crates/cyrup-tools/src/lock.rs) also do
  `Arc::ptr_eq(&a.map, &b.map)` and `b.map.get(&key).map(|e| Arc::clone(e.value()))`. Both stop
  compiling against an opaque type. Three observers are needed, not one.
- **The snippet trips a clippy warning.** A `pub fn new()` with no `Default` fires
  `clippy::new_without_default` (warn-by-default; confirmed by building the mirror crate with and
  without the attribute). Not `Default`-ing is the right call here — see point 4 above — so the
  `allow` is required, with the reason attached.

### 2. Exactly which map operations the module performs

The complete set, across all 126 lines of
[`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs):

| Op | Site | Becomes |
| --- | --- | --- |
| `entry(..).or_insert_with(..).clone()` | `:57-61` | private `get_or_insert` |
| `remove_if(&key, strong_count == 1)` | `:97` (`KeyedGuard::drop`) | private `evict_if_unreferenced` |
| `remove_if(&key, strong_count == 1)` | `:124` (`PendingEntry::drop`) | private `evict_if_unreferenced` |
| `Arc::clone(&self.map)` | `:56`, `:73` | `self.map.clone()` |

Three call sites, two private methods. Nothing else in the module reads or writes the map, so `.0`
stays confined to the `impl KeyedLockMap` block.

### 3. Exactly what the consumers need back

`grep` over both crates for `dashmap|DashMap` and for `\.map\.`:

- [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs) needs **nothing**. It
  names `KeyedLockMap<PathBuf>` in two statics and never touches the map. `Arc` becomes unused in
  the file (`:19` and `:23` are its only non-doc uses), so the import narrows to
  `use std::sync::LazyLock;`.
- [`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs) needs three observers,
  all in `mod tests` and all read-only:

  | Need | Sites | Observer |
  | --- | --- | --- |
  | membership | `:240,243,264,272,277,289,304,317` | `contains_key(&K) -> bool` |
  | same map object | `:215,216` | `ptr_eq(&Self) -> bool` |
  | the mutex behind a key | `:218,219` | `mutex_for(&K) -> Option<Arc<Mutex<()>>>` |

  Eight of those twelve sites are `locks.map.contains_key(&key)` and keep **byte-identical** call
  syntax. Only `:215-219` change. `Arc` survives in `mod tests` (`Arc::new(FileMutationLocks::new())`
  at `:168`/`:391`, `Arc::ptr_eq(&via_b, &via_c)` at `:220`) but dies in the parent module, so it
  moves from the file-level `use` into the test module's.

Read-only observers cannot break the invariant — only mutation can — so publishing them does not
reopen what the newtype closes. `mutex_for` hands back an `Arc` clone rather than a `dashmap::Ref`,
so no `dashmap` type appears in any signature; the one caveat (holding it defers eviction, exactly
as holding a `KeyedGuard` does) is documented on the method.

### 4. `dashmap` stays in the workspace table; nothing else changes

[`Cargo.toml:143`](../../Cargo.toml) `dashmap = { version = "6.2.1" }` keeps two other consumers —
`cyrup-provider` (`src/auth/store.rs`, `src/api/mod.rs`) and `cyrup-ext`. The root manifest is not
touched. `Cargo.lock` is not touched: removing a `workspace = true` edge from two members does not
change the resolved graph while other members keep it.

### 5. Formatting: the change is rustfmt-clean, and it clears three pre-existing diffs

Measured with `rustfmt 1.9.0-stable (88d9e12ae1 2026-08-18)`, `--edition 2024`, no `rustfmt.toml`
in the tree, on patched copies of the three real files:

| File | `Diff in` count before | after |
| --- | --- | --- |
| `crates/cyrup-core/src/keyed_lock.rs` | 3 | **0** |
| `crates/cyrup-tools/src/lock.rs` | 2 | **1** |
| `crates/cyrup-config/src/lock.rs` | 3 | 3 |

Every line the change writes is already in rustfmt's own shape, so no `cargo fmt` run is needed or
wanted. The three `keyed_lock.rs` diffs disappear as a side effect: two were the `remove_if` chains
(over `chain_width` 60) that `evict_if_unreferenced` replaces, and one was the `PendingEntry`
struct literal (over `struct_lit_width` 18) that is now written pre-broken. The `cyrup-tools` count
drops 2 → 1 because `FileMutationLocks::new`'s body is rewritten in the expanded form; the
survivor is the `guard()` chain at `:143`, which belongs to
[`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md).
`cyrup-config/src/lock.rs`'s three are untouched and all pre-existing (the `use` ordering at `:6`,
and two `ConfigError::Lock { .. }` literals). No new line exceeds 100 columns.

## Required change

### A. `crates/cyrup-core/src/keyed_lock.rs`

Replace `:18-19`:

```rust
/// The map one lock domain is built over.
pub type KeyedLockMap<K> = Arc<DashMap<K, Arc<Mutex<()>>>>;
```

with exactly:

```rust
/// The map one lock domain is built over.
///
/// Opaque on purpose. This module's whole safety argument is "for any live key there is exactly
/// one `Arc<Mutex<()>>`, minted only by [`KeyedLocks::guard`] and evicted only while the map holds
/// the last reference to it". A bare `Arc<DashMap<..>>` alias hands every owner `insert`,
/// `remove`, `clear`, `alter` and `entry`, each of which can put a second live mutex behind one
/// key and void mutual exclusion with no error to either party. The wrapper keeps every mutating
/// operation inside this module — `get_or_insert` and `evict_if_unreferenced` below are the only
/// two, and both are private — and leaves callers three read-only observers. It also keeps
/// `dashmap` out of consumers' manifests: nothing about a lock domain names a `dashmap` type.
pub struct KeyedLockMap<K: Eq + Hash + Clone>(Arc<DashMap<K, Arc<Mutex<()>>>>);

impl<K: Eq + Hash + Clone> KeyedLockMap<K> {
    /// A fresh, empty lock domain.
    ///
    /// Not `const fn`, and it does not need to be: `LazyLock::new` stores the initializer and
    /// runs it on first deref, so a `static M: LazyLock<KeyedLockMap<K>>` built with
    /// `LazyLock::new(KeyedLockMap::new)` is exactly as valid as the closure it replaces —
    /// `DashMap::new` is not `const fn` either.
    //
    // Deliberately NOT `Default`. `Default` on a lock domain means "a fresh, isolated one", which
    // is the bug `FileMutationLocks::default` is hand-written to avoid; with the alias, a
    // `#[derive(Default)]` on any struct holding this field compiled and silently produced that
    // second domain. Without the impl the derive is a compile error instead.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Arc::new(DashMap::new()))
    }

    /// Whether `key` currently has an entry. Observing the map cannot break the
    /// one-live-mutex-per-key invariant; only mutating it can, which is why the observers are
    /// public and the mutators are not.
    pub fn contains_key(&self, key: &K) -> bool {
        self.0.contains_key(key)
    }

    /// The mutex currently registered for `key`, if any — an `Arc` clone, never a map reference,
    /// so no `dashmap` type crosses the boundary. Holding the returned `Arc` defers eviction of
    /// the entry until it drops (the `strong_count == 1` predicate below sees it); it cannot mint
    /// a second mutex for the key.
    pub fn mutex_for(&self, key: &K) -> Option<Arc<Mutex<()>>> {
        self.0.get(key).map(|e| Arc::clone(e.value()))
    }

    /// Whether two handles name the SAME map, i.e. the same lock domain. This is the property
    /// consumers assert on: two independently constructed owners must contend.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The mutex for `key`, minting one if the key is unregistered. The ONLY insertion path.
    fn get_or_insert(&self, key: K) -> Arc<Mutex<()>> {
        self.0
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Evict `key`'s entry iff the map holds the last reference to its mutex. The ONLY removal
    /// path, and the one predicate both drop paths share.
    ///
    /// `remove_if` runs the predicate while holding the shard lock, so a concurrent
    /// [`KeyedLocks::guard`] that has just cloned the `Arc` is observed (`strong_count > 1`) and
    /// the entry is kept.
    fn evict_if_unreferenced(&self, key: &K) {
        self.0.remove_if(key, |_, v| Arc::strong_count(v) == 1);
    }
}

impl<K: Eq + Hash + Clone> Clone for KeyedLockMap<K> {
    /// An `Arc` clone of the same map — never a second lock domain. Hand-written rather than
    /// derived so that guarantee is stated where a reader looks for it.
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
```

In `KeyedLocks::guard` (`:55-61`), replace:

```rust
        let _pending = PendingEntry { map: Arc::clone(&self.map), key: key.clone() };
        let lock = self
            .map
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
```

with:

```rust
        let _pending = PendingEntry {
            map: self.map.clone(),
            key: key.clone(),
        };
        let lock = self.map.get_or_insert(key.clone());
```

In the `select!`'s success arm (`:73`), `map: Arc::clone(&self.map),` becomes
`map: self.map.clone(),`.

In `KeyedGuard::drop` (`:93-97`), replace the two comment lines and the `remove_if` call with the
single call — the `remove_if` explanation now lives on `evict_if_unreferenced`:

```rust
        self.inner.take();
        self.lock.take();
        self.map.evict_if_unreferenced(&self.key);
```

In `PendingEntry::drop` (`:119-125`), likewise:

```rust
    fn drop(&mut self) {
        // Same single eviction path as `KeyedGuard::drop`. On the SUCCESS path the returned
        // `KeyedGuard` holds a clone of the mutex, so this is a no-op and the guard's own drop
        // does the eviction.
        self.map.evict_if_unreferenced(&self.key);
    }
```

The `use dashmap::DashMap;` at `:13` stays — it is now used only inside this module, which is the
point. `#[forbid(unsafe_code)]`, the module doc, `Cancelled`, `KeyedLocks`' and `KeyedGuard`'s
declarations and every other comment are unchanged. `crates/cyrup-core/src/lib.rs:33` already
re-exports `KeyedLockMap` by name and needs no edit.

### B. `crates/cyrup-tools/src/lock.rs`

```rust
// :14   delete   use dashmap::DashMap;
// :16   use std::sync::{Arc, LazyLock};   ->   use std::sync::LazyLock;
```

`:29-30` collapses to one line (94 columns):

```rust
static FILE_MUTATION_LOCKS: LazyLock<KeyedLockMap<PathBuf>> = LazyLock::new(KeyedLockMap::new);
```

`:73-75` — the `Default` doc names a mechanism that no longer exists; replace it:

```rust
    /// Hand-written, NOT derived: `Default` here must mean "attach to the process-global map",
    /// never "a fresh empty one" — a per-owner lock domain is precisely the bug this type exists
    /// to eliminate. It must be an alias for [`FileMutationLocks::new`].
```

`:90-93` — `new()`:

```rust
    pub fn new() -> Self {
        let map = FILE_MUTATION_LOCKS.clone();
        Self {
            inner: KeyedLocks::new(map.clone()),
            map,
        }
    }
```

(`FILE_MUTATION_LOCKS.clone()` resolves through `LazyLock`'s `Deref` to `KeyedLockMap::clone` —
`LazyLock` is not itself `Clone`, so there is no ambiguity. The doc above it, "This is a cheap
`Arc` clone, not a fresh map", stays true and stays put.)

`:153` — `mod tests` gains the import the parent lost:

```rust
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
```

`:215-219` — the only test lines that change:

```rust
        // Same map object behind every handle...
        assert!(a.map.ptr_eq(&b.map));
        assert!(a.map.ptr_eq(&c.map));
        // ...and therefore the same mutex for the same path.
        let via_b = b.map.mutex_for(&key).unwrap();
        let via_c = c.map.mutex_for(&key).unwrap();
```

`:220` (`assert!(Arc::ptr_eq(&via_b, &via_c));`), `:222` (`via_b.try_lock()`) and the eight
`locks.map.contains_key(&key)` sites are unchanged.

### C. `crates/cyrup-config/src/lock.rs`

```rust
// :7    use std::sync::{Arc, LazyLock};   ->   use std::sync::LazyLock;
// :11   delete   use dashmap::DashMap;
// :19
static CONFIG_LOCKS: LazyLock<KeyedLockMap<PathBuf>> = LazyLock::new(KeyedLockMap::new);
// :23
    LazyLock::new(|| KeyedLocks::new(CONFIG_LOCKS.clone()));
```

Nothing else. The `:21` doc ("`KeyedLocks::new` is an `Arc` clone") stays true.

### D. The two manifests

Delete one line from each; add nothing, reorder nothing.

- [`crates/cyrup-tools/Cargo.toml:25`](../../crates/cyrup-tools/Cargo.toml) — `dashmap = { workspace = true }`
- [`crates/cyrup-config/Cargo.toml:22`](../../crates/cyrup-config/Cargo.toml) — `dashmap = { workspace = true }`

## Interaction with the sibling tasks — read before starting

Order does not matter, but two siblings need a mechanical adjustment depending on which lands
first.

**[`LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md`](./LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md)**
adds `impl Clone for KeyedLocks<K>` with the body `Self { map: Arc::clone(&self.map) }`. That does
not compile against the newtype. The compatible body — which is also what rustfmt produces, since
the literal exceeds `struct_lit_width` — is:

```rust
impl<K: Eq + Hash + Clone> Clone for KeyedLocks<K> {
    /// An `Arc` clone of the same map — see the type docs: never a second lock domain.
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}
```

If that task lands first, this one must convert its body. If this one lands first, that task must
be written as above. Either way the semantics are identical.

**[`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md)**
(spec already written) replaces the `map` field doc at `crates/cyrup-tools/src/lock.rs:60-63` with
a block containing the clause:

> Adding one would hand every holder of a `&KeyedLocks` the raw `DashMap`, `clear()` and `remove()`
> included, on a map whose one-live-mutex-per-key invariant is the entire safety argument of that
> module.

After the newtype there is no raw `DashMap` to hand out, so that sentence is false. If the block is
already in the file when this change is made, replace exactly that clause with:

> Adding one would hand every holder of a `&KeyedLocks` the whole lock domain, on a map whose
> one-live-mutex-per-key invariant is the entire safety argument of that module.

Everything else in that block stays correct: the field is still read by the same four tests, still
per-instance, and `FILE_MUTATION_LOCKS` is still a vacuous substitute for the identity assertion —
`a.map.ptr_eq(&b.map)` has the same problem `Arc::ptr_eq(&a.map, &b.map)` had.

**[`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)**
lists two `cyrup-tools/src/lock.rs` hunks. This change resolves the first (`new()`, its `:89`) by
rewriting that body in rustfmt's shape, and leaves the second (`guard()`, its `:143`) alone. That
task's `cyrup-config/src/settings/` half is untouched.

**[`LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md`](./LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md)**
newtypes the *guard* inside `cyrup-tools`. Disjoint from this change; no coordination needed.

## Paths considered and rejected

1. **Document the invariant, keep the alias.** Rejected on the evidence in the decision section —
   the two costs that would justify it (a `const fn` requirement, an established API) are
   respectively false and absent, and it leaves `dashmap` pinned into two manifests that have no
   other use for it. A comment cannot make `map.clear()` fail to compile.
2. **`#[cfg(test)]`-gate the observers in `cyrup-core`** (the finding's own fallback). Rejected —
   it does not work across a crate boundary; see research §1.
3. **Put the observers behind a `test-util` cargo feature.** Rejected — cargo unifies features
   across the build graph, so enabling it from `cyrup-tools`' `[dev-dependencies]` also enables it
   for the normal build. It buys exactly nothing over plain read-only methods and costs a feature
   flag in three manifests.
4. **Expose `is_locked(&self, key) -> bool` instead of `mutex_for`,** so no `Arc` escapes.
   Rejected — the only honest implementation is `try_lock().is_err()`, which momentarily *acquires*
   the mutex on the success path. Publishing a method on a fairness-sensitive concurrency primitive
   that can interleave with a queued waiter, purely to keep one test's `Arc` inside the module, is
   a worse trade than documenting `mutex_for`'s one caveat.
5. **`#[doc(hidden)]` on `mutex_for`.** Rejected — it stays callable, so it hides the caveat from
   the very reader who needs it without removing the capability. If it is public it should be
   documented; it is.
6. **Give `KeyedLocks` the observers and delete `FileMutationLocks::map`.** Rejected — it kills
   `independent_handles_share_one_lock_per_path`'s per-instance identity assertion, which is the
   sole structural check that a second `FileMutationLocks` joins the domain rather than getting an
   isolated one. Reasoned out in full in
   [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md)
   §4; that task's conclusion (keep the field, no accessor on `KeyedLocks`) is honoured here.
7. **`impl Deref<Target = DashMap<..>> for KeyedLockMap`** to avoid touching the tests. Rejected —
   it is the alias again with extra steps: `map.clear()` still resolves.
8. **Add `Default` alongside `new()`** to silence `clippy::new_without_default` without an
   `allow`. Rejected — `Default` on a lock domain is the isolated-map footgun; withholding it is
   the point (decision section, benefit 4).

## Do not touch

- `crates/cyrup-core/src/keyed_lock.rs`'s module doc (`:1-10`), `use` block (`:12-16`), `Cancelled`
  (`:21-34`), the `KeyedLocks` type doc (`:36-39`), the `biased` comment block inside `select!`,
  the `KeyedGuard` type doc, and the `PendingEntry` type doc. Only the four regions named in **A**
  change.
- `crates/cyrup-core/src/lib.rs` — the re-export at `:33` already names `KeyedLockMap`.
- Every test body in `crates/cyrup-tools/src/lock.rs` except `:215-219` and the added `use`.
  Specifically: no `contains_key` call site changes, and `mod tests`' eight of them must stay
  byte-identical.
- `crates/cyrup-tools/src/lock.rs:143` (the `guard()` chain rustfmt wants broken) — sibling task.
- `crates/cyrup-tools/src/lock.rs:60-65` — the `map` field and its `#[cfg_attr(not(test),
  allow(dead_code))]` gate; sibling task, except for the one conditional clause named above.
- The root [`Cargo.toml`](../../Cargo.toml) and `Cargo.lock`.
- No `cargo fmt` of any scope — every new line is already rustfmt's output.
- No new test, no new test module in `cyrup-core`, no `Cargo.toml` `[features]` section.

## Definition of done

1. Exactly five files modified: the three `.rs` files and the two crate manifests.
   `git diff --name-only` lists no others; `git diff -- Cargo.toml Cargo.lock` is empty.
2. `grep -rn "dashmap\|DashMap" crates/cyrup-tools crates/cyrup-config` returns **nothing**.
3. `grep -rl "dashmap\|DashMap" crates/cyrup-core/src` names only `keyed_lock.rs`, and
   `grep -rc "^use dashmap" crates/cyrup-core/src/keyed_lock.rs` returns `1` — the one import,
   which stays. (A plain `grep -c dashmap` on that file returns 7; the other six are prose in the
   new doc comments, which is expected.)
4. `grep -c "self\.0" crates/cyrup-core/src/keyed_lock.rs` returns `6`, and every hit is inside the
   `impl<K: Eq + Hash + Clone> KeyedLockMap<K>` block or its `Clone` impl — none in `KeyedLocks`,
   `KeyedGuard` or `PendingEntry`.
5. `grep -c "\.remove_if(\|\.or_insert_with(" crates/cyrup-core/src/keyed_lock.rs` returns `2`,
   both inside `KeyedLockMap`'s private methods. (Match the leading dot — `remove_if` also appears
   once as prose in `evict_if_unreferenced`'s doc.)
6. `cargo build -p cyrup-core -p cyrup-tools -p cyrup-config` is warning-free, and
   `cargo clippy -p cyrup-core -p cyrup-tools -p cyrup-config --all-targets` adds no warning that
   was not there before (in particular, no `new_without_default`).
7. `cargo test -p cyrup-tools --lib lock::` reports **8 passed, 0 failed** — the same eight tests,
   none renamed, none added.
8. `cargo test -p cyrup-config --lib` is unchanged from before the edit.
9. `rustfmt --edition 2024 --check crates/cyrup-core/src/keyed_lock.rs` is silent (0 diffs, down
   from 3); `... crates/cyrup-tools/src/lock.rs` reports exactly **one** `Diff in` (the `guard()`
   chain, sibling task, down from 2); `... crates/cyrup-config/src/lock.rs` reports exactly
   **three**, the same three as before (`use` ordering plus two `ConfigError::Lock` literals).
10. No line added by this change exceeds 100 columns:
    `awk 'length($0)>100 {print FILENAME": "NR}' crates/cyrup-core/src/keyed_lock.rs` is empty.
11. A scratch check that the closure is really shut:
    `KeyedLockMap::<std::path::PathBuf>::new().clear()` fails to compile with
    `error[E0599]: no method named `clear``, and
    `#[derive(Default)] struct S { m: cyrup_core::KeyedLockMap<std::path::PathBuf> }` fails with
    `error[E0277]: the trait bound `KeyedLockMap<PathBuf>: Default` is not satisfied`. Delete the
    scratch file afterwards.
12. `cargo doc -p cyrup-core --no-deps` emits no `unresolved link` warning for
    `KeyedLocks::guard` (linked twice from the new doc).

## Not in this task

- `impl Clone for KeyedLocks<K>` — [`LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md`](./LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md).
- The `map` field's doc comment — [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md).
- `MutationGuard` as a newtype — [`LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md`](./LOW-mutationguard-alias-erases-the-lock-domain-type-distinction.md).
- The remaining rustfmt hunks — [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md).
- Giving `cyrup_core::keyed_lock` tests of its own. It still has none; that gap is real and belongs
  in its own task.
