---
title: Map Field Doc Claims A Property The Diff Refutes
priority: MEDIUM
stage: aug
status: done
updated: 2026-08-23 03:36
---

# The `map` field's justification is wrong twice; replace the doc comment

One change, one file, one hunk: rewrite the doc comment on `FileMutationLocks::map` in
[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs) (`:60-63`). The field
stays. The `#[cfg_attr(not(test), allow(dead_code))]` gate stays. No test changes, no `cyrup-core`
changes, no formatting pass.

The comment is the only thing standing between a future maintainer and deleting a field that four
tests depend on, and both of its load-bearing clauses are false. Correcting it **is** the
deliverable — it is not documentation polish, it is repairing the sole guard on a deletion that
would silently drop the workspace's only coverage of the extracted lock registry.

## Research — what was verified

### 1. "rather than reached through `inner`" describes a route that does not exist

[`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) `:40-78` is the
complete surface of `KeyedLocks<K>`:

```rust
pub struct KeyedLocks<K: Eq + Hash + Clone> {
    map: KeyedLockMap<K>,          // :41 — private
}

impl<K: Eq + Hash + Clone> KeyedLocks<K> {
    pub fn new(map: KeyedLockMap<K>) -> Self { ... }                     // :46
    pub async fn guard(&self, key: K, cancel: &CancelToken) -> ...       // :54
}
```

`map` is private, there is no accessor, no `Deref`, no `AsRef`, no `Clone`, no public field. The
whole file is 126 lines and contains exactly those two `pub fn`s. So the field was never a choice
between two available routes — reaching through `inner` was never on the table.

Confirmed the current doc claims otherwise, verbatim at
[`lock.rs:60`](../../crates/cyrup-tools/src/lock.rs):
`/// The same map `inner` is built over. Kept as a field, rather than reached through `inner`,`

### 2. "with no test changes at all" is false as a claim about the diff — and the finding's own evidence line was also wrong

The diff does touch the test bodies. Five hunks inside `mod tests` are rewritten
(`:239-245`, `:268-274`, `:326-341`, `:374-386`, `:390-398` in the new file). `git diff --stat`
against the merge base reports `55 insertions(+), 84 deletions(-)`.

**The prior finding's verification command does not show what it claims.** `git diff -w` does *not*
leave the test region empty — `--ignore-all-space` normalises whitespace *within* a line, but the
reflow re-splits single lines into three or four, which is a genuine line-set change. Running
`git diff -w 4902cddf -- crates/cyrup-tools/src/lock.rs` still prints all five test hunks. Anyone
who re-runs the command in the finding will conclude the finding is wrong.

Two commands that do prove the point, both run and both green:

```
# a) token-identical: strip all whitespace from each `mod tests` and compare
$ git show 4902cddf:crates/cyrup-tools/src/lock.rs > /tmp/base-lock.rs
$ for f in /tmp/base-lock.rs crates/cyrup-tools/src/lock.rs; do
    awk '/^#\[cfg\(test\)\]/{p=1} p' "$f" | tr -d ' \n\t'; echo; done | uniq | wc -l
1                      # → one distinct line: 8794 bytes, byte-for-byte identical

# b) stronger: the new test module IS rustfmt's output on the old one
$ cp /tmp/base-lock.rs /tmp/base-fmt.rs && rustfmt --edition 2024 /tmp/base-fmt.rs
$ diff <(awk '/^#\[cfg\(test\)\]/{p=1} p' /tmp/base-fmt.rs) \
       <(awk '/^#\[cfg\(test\)\]/{p=1} p' crates/cyrup-tools/src/lock.rs)
                       # → empty
```

So: the reflow is a plain `cargo fmt -p cyrup-tools` pass reproduced exactly by stock
`rustfmt --edition 2024` with no `rustfmt.toml` anywhere in the tree (defaults; `fn_call_width` =
60 is what breaks the `assert!(..)` calls). Behaviour is untouched. The *property* the comment
means to assert is true; the *sentence* it uses to assert it — a claim about a diff — is not.

**Decision: restate as a property, not as a claim about a diff.** "No test needed a behavioural
change" is true, is checkable forever, and does not rot the moment the commit scrolls into history.
Restoring the merge-base formatting to make the literal sentence true is the wrong direction (see
*Paths considered and rejected*).

### 3. The field is load-bearing, and more so than the finding says

Twelve read sites across four tests, all in
[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs):

| Test | Lines | Reads |
| --- | --- | --- |
| `independent_handles_share_one_lock_per_path` | `:215-219` | `a.map`, `b.map`, `c.map` |
| `guard_evicts_its_entry_on_drop` | `:240,243` | `locks.map` |
| `a_cancelled_acquisition_evicts_its_entry_instead_of_leaking_it` | `:264,272,277,289` | `locks.map` |
| `dropping_the_acquisition_future_evicts_its_entry` | `:304,317` | `locks.map` |

And these are the **only** tests in the workspace that observe the extracted registry's map:

- `crates/cyrup-core/src/keyed_lock.rs` has no `mod tests` and no integration test
  (`crates/cyrup-core/tests/` does not exist).
- `crates/cyrup-config/src/lock.rs`, the other `KeyedLocks` consumer, has no `mod tests` either;
  nothing anywhere else greps for `strong_count` or map membership against a lock domain.

So eviction on guard drop, eviction on a cancelled acquisition, eviction on a dropped acquisition
future, and the `biased` cancel-race determinism are covered in exactly one place, through this
field. Deleting it deletes all four.

### 4. The `FILE_MUTATION_LOCKS` static is a *partial* substitute — this is the cleanup the current comment fails to warn against

`mod tests` does `use super::*`, so the private static
[`FILE_MUTATION_LOCKS`](../../crates/cyrup-tools/src/lock.rs) (`:29-30`) is already in scope. The
three eviction tests could swap `locks.map.contains_key(&key)` for
`FILE_MUTATION_LOCKS.contains_key(&key)` with no loss.

`independent_handles_share_one_lock_per_path` cannot. Its point is per-*instance* identity:

```rust
assert!(Arc::ptr_eq(&a.map, &b.map));   // :215
assert!(Arc::ptr_eq(&a.map, &c.map));   // :216 — c is FileMutationLocks::default()
```

Against the static those assertions become `ptr_eq(static, static)` and pass vacuously. The test
exists to catch a `FileMutationLocks` that quietly builds a fresh map — the isolated-lock-domain
bug the type doc at `:54-58` calls "precisely the bug this type exists to prevent", and the reason
`Default` at `:72-79` is hand-written. Without a per-instance handle, no structural assertion can
see it; only a timing/contention test could, which is the style the suite deliberately avoids
(`:249-252`: "Structural, not timing-based").

That is the real, narrow reason the field exists, and the corrected comment must say it.

### 5. The gate is correct and non-fatal

`new()` at `:90-93` writes the field; nothing reads it outside `cfg(test)`, so rustc's `dead_code`
lint fires without the attribute. There is no `deny(warnings)` and no `deny(dead_code)`:
`crates/cyrup-tools/src/lib.rs:16` denies only `unsafe_code`, and `[workspace.lints.clippy]` in the
root `Cargo.toml` denies only `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`. Holding two
`Arc` clones of one `DashMap` does not perturb eviction: the `Arc::strong_count(v) == 1` predicate
in `KeyedGuard::drop` (`keyed_lock.rs:97`) and `PendingEntry::drop` (`:124`) counts the *values*
(`Arc<Mutex<()>>`), never the map handle. Nothing about the field or the gate changes.

## Required change — one file, one hunk

[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs), lines `60-63`.

Replace:

```rust
    /// The same map `inner` is built over. Kept as a field, rather than reached through `inner`,
    /// because the tests in this file assert on map membership directly — which is what lets the
    /// move of the registry mechanics into `cyrup-core` be proven with no test changes at all.
    /// Hence the gate: outside `cfg(test)` nothing reads it, and that is the intended state.
    #[cfg_attr(not(test), allow(dead_code))]
    map: KeyedLockMap<PathBuf>,
```

with exactly:

```rust
    /// A second handle on the map `inner` is built over, held **per instance** so the tests below
    /// can assert on map membership and on map *identity*. Four tests read it —
    /// `independent_handles_share_one_lock_per_path`, `guard_evicts_its_entry_on_drop`,
    /// `a_cancelled_acquisition_evicts_its_entry_instead_of_leaking_it` and
    /// `dropping_the_acquisition_future_evicts_its_entry` — and they are the only coverage of
    /// entry eviction, the dropped-future gap and the `biased` cancel race in the workspace,
    /// because [`cyrup_core::keyed_lock`] carries no tests of its own.
    ///
    /// Two cleanups look available here and are not:
    ///
    /// - *Reach through `inner`.* There is no route. [`KeyedLocks`] keeps its map private and
    ///   exposes only `new` and `guard` — no accessor, no `Deref`. Adding one would hand every
    ///   holder of a `&KeyedLocks` the raw `DashMap`, `clear()` and `remove()` included, on a map
    ///   whose one-live-mutex-per-key invariant is the entire safety argument of that module.
    /// - *Use the `FILE_MUTATION_LOCKS` static.* It is in scope for the tests and would serve the
    ///   three eviction tests, but it is one object by construction, so it cannot express
    ///   `Arc::ptr_eq(&a.map, &b.map)`: the check that a separately constructed
    ///   `FileMutationLocks` joins this lock domain instead of silently getting an isolated one —
    ///   precisely the bug this type exists to prevent, and the reason `Default` below is
    ///   hand-written.
    ///
    /// Holding the map per instance is also what let the registry mechanics move into
    /// [`cyrup_core::keyed_lock`] with no test needing a behavioural change.
    ///
    /// Outside `cfg(test)` nothing reads it, and that is the intended state; hence the gate.
    #[cfg_attr(not(test), allow(dead_code))]
    map: KeyedLockMap<PathBuf>,
```

Notes on the text, so it is not "improved" back into being wrong:

- No sentence claims anything about a diff or a commit. The property is "no test needed a
  behavioural change", which stays true after the commit scrolls away. Do **not** re-add "with no
  test changes at all" in any wording — the branch reflowed five test hunks and that is fine.
- The two rejected cleanups are named explicitly because each is the plausible next move, and
  neither is discoverable from the field alone. That is the whole job of this comment.
- `[`KeyedLocks`]` and `[`cyrup_core::keyed_lock`]` both resolve: `KeyedLocks` is imported at
  `:12` and `cyrup_core::keyed_lock` is already linked at `:67`.
- All lines are ≤ 100 columns; rustfmt does not reflow comments under the default config, so this
  block is stable under `cargo fmt`.

## Design question — keep the field, do not add an accessor

Answered deliberately, not by default:

**Keep the field.** `cyrup-core` should not grow a `KeyedLocks::map()` accessor. The consumer
already owns the map — it declares the `FILE_MUTATION_LOCKS` static and passes a clone into
`KeyedLocks::new` — so an accessor would be `cyrup-core` re-exporting state the caller handed it,
purely to save one `Arc` clone in a downstream test. It also cuts directly against
[`LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md`](./LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md),
which argues the raw `Arc<DashMap<..>>` alias is already too much surface; an accessor would make
every holder of a `&KeyedLocks` — not just the domain owner — able to `clear()` or `remove()` the
map and break mutual exclusion with no error to either party.

The cost of keeping it is one gated, test-only field on one type, already paid and already sound.
The benefit is the per-instance identity assertion in
`independent_handles_share_one_lock_per_path`, which nothing else in the workspace can express.
The corrected comment records that trade so the next reader does not have to rediscover it.

## Paths considered and rejected

1. **Add `pub fn map(&self) -> &KeyedLockMap<K>` to `KeyedLocks`, delete the field.** Rejected —
   widens `cyrup-core`'s public API for a downstream test-only need, and hands the raw `DashMap`
   mutating surface to every `&KeyedLocks` holder. See above.
2. **Add a `#[cfg(test)]` accessor to `KeyedLocks`.** Rejected — it does not work. `cfg(test)` is
   set per compilation unit; when `cyrup-tools` builds its test target, `cyrup-core` is still built
   as a normal dependency with `cfg(test)` off, so the accessor would not exist. Making it real
   requires a cargo feature (`test-util`), i.e. a new feature and a dev-dependency edge for one
   assertion.
3. **Rewrite the four tests onto `FILE_MUTATION_LOCKS`, delete the field.** Rejected — loses the
   per-instance `Arc::ptr_eq` assertion (§4), and rewrites the exact tests whose unchanged bodies
   are the extraction's evidence, for zero behavioural gain and a larger diff.
4. **Move the eviction tests down into `cyrup_core::keyed_lock`.** Not rejected on the merits — it
   is a genuinely good idea, and §3 shows `keyed_lock` has no tests at all — but it is a different
   task. It would not remove the need for this field either: domain-sharing across two
   `FileMutationLocks` instances is a `cyrup-tools` property and cannot be tested in `cyrup-core`.
   File it separately if wanted; it is out of scope here.
5. **Restore the merge-base formatting of the five test hunks so "no test changes at all" reads
   true.** Rejected — reverting formatting to rescue a sentence is backwards, and it contradicts
   [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md),
   which goes the other way (finish the `cargo fmt` pass over the two hunks the branch left dirty:
   `lock.rs:92` and `:143`). Fix the sentence, not the whitespace.

## Do not touch

- [`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) — no new
  accessor, no visibility change, no new `pub` item, no feature flag.
- `mod tests` in [`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs)
  (`:150-420`) — not one line, including the five reflowed hunks.
- The `#[cfg_attr(not(test), allow(dead_code))]` attribute and the `map` field declaration itself.
- The doc on the `inner` field (`:66-68`), the type doc (`:51-58`), `Default` (`:72-79`) and
  `new()` (`:88-93`).
- `crates/cyrup-tools/src/lock.rs:92` and `:143` — the two lines rustfmt would reflow. They belong
  to the sibling rustfmt task; leaving them alone keeps this diff to a single hunk.
- No `cargo fmt` of any scope. This change adds only `///` lines, which stock rustfmt leaves alone.

## Definition of done

1. `crates/cyrup-tools/src/lock.rs` is the only modified file, and
   `git diff --stat` on it shows one hunk in the `pub struct FileMutationLocks` block. Nothing
   below `#[cfg(test)]` moved:
   `diff <(git show HEAD:crates/cyrup-tools/src/lock.rs | awk '/^#\[cfg\(test\)\]/{p=1} p') <(awk '/^#\[cfg\(test\)\]/{p=1} p' crates/cyrup-tools/src/lock.rs)`
   is empty.
2. `grep -n "reached through \`inner\`\|no test changes at all" crates/cyrup-tools/src/lock.rs`
   returns nothing.
3. `grep -c "no test needed a behavioural change" crates/cyrup-tools/src/lock.rs` returns `1`.
4. `git diff -- crates/cyrup-core/` is empty.
5. `cargo check -p cyrup-tools --lib` is warning-free (the `dead_code` gate still holds) and
   `cargo test -p cyrup-tools --lib lock::` still reports 8 passed, 0 failed.
6. `cargo doc -p cyrup-tools --no-deps` emits no `unresolved link` warning for
   `KeyedLocks` or `cyrup_core::keyed_lock`.
7. `rustfmt --edition 2024 --check crates/cyrup-tools/src/lock.rs` reports the **same two**
   pre-existing diffs it reported before this change (`:92` and the `:143` chain) and no others —
   i.e. this change neither fixes nor adds a formatting violation.

## Not in this task

- Newtyping `KeyedLockMap` — [`LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md`](./LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md).
- Formatting the two dirty production lines — [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md).
- `KeyedLocks`' own doc claiming a `Clone` it does not have — [`LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md`](./LOW-keyedlocks-doc-promises-a-clone-that-does-not-exist.md).
- Giving `cyrup_core::keyed_lock` tests of its own (§3, path 4) — worth filing, not done here.
