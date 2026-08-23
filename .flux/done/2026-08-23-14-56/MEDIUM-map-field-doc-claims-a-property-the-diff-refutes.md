---
title: Map Field Doc Claims A Property The Diff Refutes
priority: MEDIUM
stage: done
status: done
updated: 2026-08-23 (closed out)
---

# Two clauses in the replacement doc are now false against `cyrup-core`

QA rating: **6/10**. The original defect is fixed — but the replacement text contains two
statements that are false against the tree as it stands, and one of them is a safety-design claim
that the module it describes contradicts head-on. This queue exists to remove false claims from
comments, so these are defects, not polish.

Almost certainly a merge-order interaction rather than carelessness:
`LOW-keyed-lock-map-alias-exposes-the-raw-dashmap` landed too, and it invalidated the rationale
this comment was written against. Judge the file, not the history — on disk the text is wrong.

**Everything else was verified and is settled. Do not redo it (see *Settled — do not re-verify*).**

## Outstanding item 1 — the `DashMap` clause is false

`crates/cyrup-tools/src/lock.rs:68-71`, bullet one of "Two cleanups look available here and are
not":

```rust
    /// - *Reach through `inner`.* There is no route. [`KeyedLocks`] keeps its map private and
    ///   exposes only `new` and `guard` — no accessor, no `Deref`. Adding one would hand every
    ///   holder of a `&KeyedLocks` the raw `DashMap`, `clear()` and `remove()` included, on a map
    ///   whose one-live-mutex-per-key invariant is the entire safety argument of that module.
```

Sentences one and two are TRUE and verified — keep them verbatim. Sentence three is false three
ways:

1. `KeyedLockMap` is no longer an `Arc<DashMap<..>>` alias. It is an opaque newtype:
   `crates/cyrup-core/src/keyed_lock.rs:34` —
   `pub struct KeyedLockMap<K: Eq + Hash + Clone>(Arc<DashMap<K, Arc<Mutex<()>>>>);` — with a
   private tuple field, no `Deref`, no `AsRef`.
2. There is no public `clear()` or `remove()` to hand out. `KeyedLockMap`'s entire public surface
   is `new` (`:49`), `contains_key` (`:56`), `mutex_for` (`:64`), `ptr_eq` (`:70`) and a
   hand-written `Clone` (`:93-99`). The only two mutators, `get_or_insert` (`:75`) and
   `evict_if_unreferenced` (`:88`), are **private**. An accessor returning `&KeyedLockMap<K>`
   would hand out three read-only observers and nothing else.
3. `dashmap` is not even a dependency of `cyrup-tools` (`grep -n dashmap crates/*/Cargo.toml` hits
   only `crates/cyrup-core/Cargo.toml:18`), so no `cyrup-tools` holder could name the type.

Worse, `keyed_lock.rs`'s own module doc (`:26-33`) says the opposite in as many words: "A bare
`Arc<DashMap<..>>` alias hands every owner `insert`, `remove`, `clear`, `alter` and `entry` … The
wrapper keeps every mutating operation inside this module … and leaves callers three read-only
observers." A maintainer trusting the `lock.rs` clause would walk away believing the exact thing
`keyed_lock.rs` was rewritten to stop being true.

**What to write instead.** The rejection still stands; only its reason has to change. Keep it a
property of the code, not of a diff. Something in this shape (wording is yours; the facts above
are not):

- there is no accessor and no `Deref`, so nothing to reach through;
- adding one to `cyrup-core` for a downstream test-only need is the wrong direction — the consumer
  already owns the map (`FILE_MUTATION_LOCKS` is declared here and a clone is handed to
  `KeyedLocks::new`), so an accessor would be `cyrup-core` re-exporting state its caller supplied;
- and `mutex_for` hands back an `Arc` clone that defers eviction while held
  (`keyed_lock.rs:60-66`), so the observers are not free of consequence either.

Do **not** replace it with a claim about what a hypothetical accessor's return type would expose
unless you re-read `keyed_lock.rs` and the claim matches it.

## Outstanding item 2 — the cited assertion does not exist and would not compile

`crates/cyrup-tools/src/lock.rs:72-77`, bullet two:

```rust
    ///   three eviction tests, but it is one object by construction, so it cannot express
    ///   `Arc::ptr_eq(&a.map, &b.map)`: the check that a separately constructed
```

The bullet's substance is correct and stays. The cited expression is wrong: `a.map` is a
`KeyedLockMap<PathBuf>`, not an `Arc`, so `Arc::ptr_eq(&a.map, &b.map)` does not type-check. The
real assertions are at `crates/cyrup-tools/src/lock.rs:252-253`:

```rust
        assert!(a.map.ptr_eq(&b.map));
        assert!(a.map.ptr_eq(&c.map));
```

Replace the citation with `a.map.ptr_eq(&b.map)`. This is a false code citation in a comment whose
entire job is to be citable, not a stale line number.

## Constraints for the rework — unchanged

- **Review-only elsewhere.** The single permitted edit is inside the doc block on
  `FileMutationLocks::map`, `crates/cyrup-tools/src/lock.rs:58-82`. The
  `#[cfg_attr(not(test), allow(dead_code))]` attribute (`:83`), the field declaration (`:84`), and
  every line of `mod tests` (from `#[cfg(test)]` at `:186` to EOF) stay untouched.
- **`crates/cyrup-core/src/keyed_lock.rs` is not modified.** No accessor, no visibility change, no
  feature flag. It is 202 lines, sha256
  `47ab2ac5b333b5233b03d4fcdf7102a0ee0ac4aa844714275e33d82e2a76e0f4`, and must still be.
- **Never re-add a claim about a diff or a commit.** The sentence "Holding the map per instance is
  also why no test needed a behavioural change when the registry mechanics moved into
  [`cyrup_core::keyed_lock`]" (`:79-80`) is the correct durable form. Keep it. Do not reintroduce
  "with no test changes at all" in any wording.
- **Stale metadata in the previous revision of this task — ignore all of it.** The file is now
  **445 lines**, not 429; sha256 is
  `764c7458dd8042a8712c576e9a84122c0b3e6ec4ae544a1c653dc458635356dd`, not `41c2f39f…`. Every line
  number in the old body predates the sibling merges. Re-read from disk; anchor on item names.
- **rustfmt is now clean on this file.** `rustfmt --edition 2024 --check crates/cyrup-tools/src/lock.rs`
  prints nothing — the two pre-existing violations the old body told you to preserve were fixed by
  the sibling rustfmt task. Do not reintroduce them, and do not run `cargo fmt` at any scope; `///`
  lines are all you are adding.
- No git command at any step. Scratch files only under `/home/user/cyrup/tmp/`.

## Definition of done

1. `grep -c 'raw `DashMap`' crates/cyrup-tools/src/lock.rs` → `0`, and no sentence in the block
   attributes `clear()`, `remove()` or a raw `DashMap` to what an accessor would expose.
2. `grep -c 'Arc::ptr_eq(&a.map' crates/cyrup-tools/src/lock.rs` → `0`;
   `grep -c 'a.map.ptr_eq(&b.map)' crates/cyrup-tools/src/lock.rs` → `2` (the comment citation plus
   the existing assertion at `:252`).
3. `grep -c 'reached through `inner`' …` → `0`; `grep -c 'no test changes at all' …` → `0`;
   `grep -c 'no test needed a behavioural change' …` → `1`. (Already true; keep it that way.)
4. `sha256sum crates/cyrup-core/src/keyed_lock.rs` still prints `47ab2ac5b333b5233b03d4fcdf7102a0ee0ac4aa844714275e33d82e2a76e0f4`.
5. `awk '/^#\[cfg\(test\)\]/{p=1} p' crates/cyrup-tools/src/lock.rs | sha256sum` is unchanged from
   before your edit (capture it first; the region is 260 lines).
6. `rustfmt --edition 2024 --check crates/cyrup-tools/src/lock.rs` prints **nothing**.
7. `cargo check -p cyrup-tools --lib` succeeds with no warnings — in particular no `dead_code` on
   `map`, proving the gate survived.
8. Intra-doc links resolve. `cargo doc -p cyrup-tools --no-deps` currently FAILS for an unrelated
   reason (see below), so use:
   `RUSTDOCFLAGS="--document-private-items -W rustdoc::broken_intra_doc_links -A rustdoc::private_intra_doc_links" cargo doc -p cyrup-tools --no-deps`
   after `rm -rf target/doc/cyrup_tools`, and confirm the only `unresolved link` warning is the
   pre-existing `ops/local/guard.rs:44` one. `--document-private-items` is required: `map` is a
   private field, so a plain `cargo doc` never lints its links at all.

## Settled — verified during QA, do not re-verify or re-litigate

- **The original defect is fixed.** Both false clauses of the old comment are gone
  (`grep -c 'reached through `inner`'` → 0, `grep -c 'no test changes at all'` → 0) and the durable
  property is present (→ 1).
- **The four test names are exact and complete.** `independent_handles_share_one_lock_per_path`
  (`:252`, `:253`, `:255`, `:256`), `guard_evicts_its_entry_on_drop` (`:277`, `:280`),
  `a_cancelled_acquisition_evicts_its_entry_instead_of_leaking_it` (`:301`, `:309`, `:314`, `:326`)
  and `dropping_the_acquisition_future_evicts_its_entry` (`:341`, `:354`) are the only readers of
  `self.map`.
- **"Only coverage in the workspace" is true.** `keyed_lock.rs` contains no `#[cfg(test)]`;
  `crates/cyrup-core/tests/` does not exist; `crates/cyrup-config/src/lock.rs` — the only other
  `KeyedLocks` consumer — has no `#[cfg(test)]` and `crates/cyrup-config/tests/` does not exist; no
  file outside `keyed_lock.rs` and `cyrup-tools/src/lock.rs` calls `contains_key`, `mutex_for` or
  `ptr_eq` on a `KeyedLockMap`.
- **"[`KeyedLocks`] keeps its map private and exposes only `new` and `guard` — no accessor, no
  `Deref`" is TRUE.** `keyed_lock.rs:120-122` (private `map` field), `:126` (`new`), `:134`
  (`guard`); no `Deref`/`AsRef` impl anywhere in the file.
- **"`FILE_MUTATION_LOCKS` … is in scope for the tests and would serve the three eviction tests"
  is TRUE.** `mod tests` does `use super::*` (`:189`), the static is declared at `:28`, and
  `contains_key` is public on `KeyedLockMap`.
- **"the reason `Default` below is hand-written" is TRUE** — `impl Default for FileMutationLocks`
  at `:91-98`, immediately below the field, aliasing `Self::new()`.
- **The gate is intact and correct.** `#[cfg_attr(not(test), allow(dead_code))]` at `:83`;
  `cargo check -p cyrup-tools --lib` is clean with zero warnings.
- **Both intra-doc links in the block resolve.** With `--document-private-items`, the only
  unresolved link in the whole crate is `ops/local/guard.rs:44`
  (`crate::ops::local::tracking::TRACKED_DETACHED_CHILD_PIDS`) — a pre-existing break from the
  `ops/local.rs` decomposition, out of scope here, but note it currently makes
  `cargo doc -p cyrup-tools --no-deps` fail outright under `broken_intra_doc_links = "deny"`. Worth
  filing separately; do not fix it in this task.
- **rustfmt is clean on `lock.rs`** and `mod tests` was not disturbed by the exec step.

## Not in this task

- The `ops/local/guard.rs:44` broken intra-doc link — separate, file it.
- Newtyping `KeyedLockMap` — already done and merged; that is what invalidated item 1.
- `KeyedLocks`' own doc claiming a `Clone` it does not have —
  [`LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md`](./LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md).
- Giving `cyrup_core::keyed_lock` tests of its own — worth filing; owned by another team.
