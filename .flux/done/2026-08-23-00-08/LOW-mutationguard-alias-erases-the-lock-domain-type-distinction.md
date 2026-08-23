---
title: Mutationguard Alias Erases The Lock Domain Type Distinction
priority: LOW
stage: qa
status: completed
updated: 2026-08-23 08:01
---

# `MutationGuard` must go back to being a nominal type: a newtype over `KeyedGuard<PathBuf>`

> **Citation policy for this file.** Every `file:line` below was re-verified against the working
> tree on 2026-08-23. Where a pointer can be given by *name* it is given by name, because a name
> cannot rot. The two edits in [Required change](#required-change) are anchored on **exact source
> text**, not on line numbers, and each anchor has been measured to occur exactly once.

## Problem

[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs) line 85 currently reads:

```rust
pub type MutationGuard = KeyedGuard<PathBuf>;
```

Historically this name was `pub struct MutationGuard { inner, lock, map, key }` with one `Drop`
impl, four private fields, and no inherent methods. The extraction into
[`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) moved those four
fields and that `Drop` into `KeyedGuard<K>` and left an alias behind. (That history is recorded in
[`LOW-public-api-changes-beyond-the-async-keyword.md`](./LOW-public-api-changes-beyond-the-async-keyword.md)
§(c); it is background, not something this task needs to re-derive, and nothing below depends on
reading it.)

Two properties were lost, both about the *name*, neither about the mechanism:

1. **The lock domain is no longer encoded in the type.** `cyrup-config` instantiates the same
   generic over the same key type for a deliberately separate map:
   [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs) declares
   `static CONFIG_LOCKS: LazyLock<KeyedLockMap<PathBuf>>` (line 20) and
   `FileLock::_in_process: KeyedGuard<PathBuf>` (line 78), under a comment (lines 17-19) stating
   that config paths and tool-mutated paths *are* different key spaces.
   `cyrup_tools::lock::MutationGuard` and that field are now literally the same Rust type. A future
   `fn commit(_: MutationGuard)` would accept a guard proving exclusion over the wrong key space,
   silently: both guards lock, both drop correctly, and neither excludes the other.
2. **Inherent impls are E0116 inside `cyrup-tools`.** `impl MutationGuard { … }` is
   "cannot define inherent impl for a type outside of the crate where the type is defined". Nothing
   is blocked outright — a local trait can still be implemented for it — but every future addition
   has to be pushed into `cyrup-core` for all domains, or bolted on via an extension trait.

Nothing about the alias is *wrong* today. What is wrong is that a change advertised as behaviour-
and API-preserving quietly downgraded a nominal type to a structural one on the crate's most
concurrency-critical name, and the downgrade is invisible because no caller spells the type.

## Research

All commands below were run in `/home/user/cyrup` on the pinned toolchain
(`rustc 1.98.0 (88d9e12ae 2026-08-18)`, `rustfmt 1.9.0-stable`, edition 2024, and **no**
`rustfmt.toml` / `.rustfmt.toml` anywhere in the tree — stock defaults).

### 1. Nothing outside `lock.rs` names the type — re-verified

```
$ grep -rn "MutationGuard" --include=*.rs crates/ | grep -v CompletionMutationGuard
crates/cyrup-tools/src/lock.rs:85   pub type MutationGuard = KeyedGuard<PathBuf>;
crates/cyrup-tools/src/lock.rs:144  ) -> Result<MutationGuard, ToolError> {
```

**Exactly two hits.** (An earlier revision of this file listed a third at `:81` for the guard's doc
comment — that doc comment does not contain the string `MutationGuard` and never did. Corrected.)
The unrelated `CompletionMutationGuardResult` in `cyrup-ext-subagents` is a different name in a
different crate and is not affected by anything here.

Both mutators bind the guard anonymously —
[`tools/write.rs`](../../crates/cyrup-tools/src/tools/write.rs) line 102 and
[`tools/edit.rs`](../../crates/cyrup-tools/src/tools/edit.rs) line 223 both say
`let _guard = self.locks.guard(&abs, &cancel).await?;`.
[`lib.rs`](../../crates/cyrup-tools/src/lib.rs) line 43 re-exports only `FileMutationLocks`
(`pub use lock::FileMutationLocks;`); the guard is reachable solely as
`cyrup_tools::lock::MutationGuard` because `pub mod lock;` (line 23) is unchanged. So the fix below
is a zero-call-site change.

### 2. A newtype has literally nothing to forward

`KeyedGuard<K>` — declared in [`keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) at lines
82-87, with its sole `impl Drop` at lines 89-99 — has **no inherent methods and no trait impls
other than `Drop`**; all four fields (`inner`, `lock`, `map`, `key`) are private to `cyrup-core`.
Its entire public surface is "exists, then drops". The "forwarding boilerplate" cost normally
charged against a newtype is therefore exactly zero here — there is no method, no `Deref`, no
`AsRef` to write.

That is also why the alias leaks only *identity*, not *capability*: unlike `KeyedLockMap`, a holder
of a `KeyedGuard` cannot reach the map or the mutex and cannot break exclusion. This finding is
what keeps the present issue LOW rather than MEDIUM.

### 3. The newtype restores the pre-extraction public API exactly

`pub struct MutationGuard(KeyedGuard<PathBuf>)` with a private field is, from outside the crate,
indistinguishable from the pre-extraction `pub struct MutationGuard { … }`:

| Property | pre-extraction struct | `pub type` alias | Proposed newtype |
| --- | --- | --- | --- |
| Nominal (distinct from every other type) | yes | **no** | yes |
| Constructible outside `cyrup-tools` | no | no (`KeyedGuard`'s fields are private) | no |
| Same type as `cyrup-config`'s guard | no | **yes** | no |
| `impl MutationGuard` legal in `cyrup-tools` | yes | **no (E0116)** | yes |
| `Send`/`Sync`/`Unpin` | from the four fields | same | same (auto traits are structural through a newtype) |
| Drop effect / ordering | `Drop` body | field's `Drop` | field's `Drop` |

`FileMutationLocks::guard`'s signature text is unchanged by this task and is recorded as
byte-identical to merge base in
[`LOW-public-api-changes-beyond-the-async-keyword.md`](./LOW-public-api-changes-beyond-the-async-keyword.md)
(its §"(d) Body changed" row for `cyrup_tools::lock::FileMutationLocks::guard`):
`pub async fn guard(&self, path: &Path, cancel: &CancelToken) -> Result<MutationGuard, ToolError>`.
On disk today that signature spans `lock.rs` lines 140-144. So the newtype reduces this branch's
public-API delta for the item to **zero**, which directly retires the `MutationGuard` row of that
file's "(c) Type changed — breaking" table.

Drop semantics are unchanged and need no `Drop` impl on the wrapper: a struct with no `Drop` of its
own drops its fields in declaration order, so the single field's `KeyedGuard::drop` runs — same
mutex release, same `remove_if(strong_count == 1)` eviction, same `PendingEntry` interaction.
Adding a `Drop` impl to the wrapper would be strictly worse (it would forbid destructuring inside
`lock.rs` for no gain) and, per §4, would not even silence the warning it looks like it should.

### 4. The obvious newtype emits a `dead_code` warning — this is the one real trap

The field is written (by the constructor) and never read, and `rustc` warns on that **even when the
field's type has a `Drop` impl**. Re-measured on the pinned toolchain against three standalone
single-file crates compiled with `rustc --edition 2024 --crate-type lib` (no `#![allow]` of any
kind), each wrapping an `Inner` that *does* have a `Drop` impl:

| Form | `warning: field `0` is never read`? | Verdict |
| --- | --- | --- |
| `pub struct W(Inner);` | **yes** | the trap |
| `pub struct W(Inner); impl Drop for W { fn drop(&mut self) {} }` | **yes** | rejected — the outer `Drop` does **not** suppress it |
| `pub struct W(#[expect(dead_code, reason = "held for its Drop")] Inner);` | no | **chosen** |
| `pub struct W(pub Inner);` | no | rejected — a `pub` field lets any crate forge or unwrap a guard |
| `pub struct W { _inner: Inner }` (`_`-prefix) | no | rejected — costs a named field plus `.map(\|g\| W { _inner: g })`, and silences by lint quirk rather than by statement |

`#[expect]` is additionally self-cancelling: adding `pub fn r(w: &W) -> &Inner { &w.0 }` to the
third crate makes rustc emit `warning: this lint expectation is unfulfilled`. Verified. That is the
property that makes it the right choice over `allow` — the annotation deletes itself the day
someone gives the field a reader.

`#[expect]` is stable since 1.81; the workspace pins `rust-version = "1.96"`
([`Cargo.toml`](../../Cargo.toml) line 89) and there is one existing use in-tree at
[`crates/cyrup-intercom/src/transport/protocol.rs`](../../crates/cyrup-intercom/src/transport/protocol.rs)
line 171. `dead_code` is warn-only here — `[workspace.lints.clippy]`
([`Cargo.toml`](../../Cargo.toml) lines 97-102) sets clippy lints only, and
`[workspace.lints.rustdoc]` (lines 106-114) rustdoc lints only; there is no `[workspace.lints.rust]`
table at all. But the file already annotates a deliberately-unread field two items above —
`#[cfg_attr(not(test), allow(dead_code))]` on `FileMutationLocks::map`
([`lock.rs`](../../crates/cyrup-tools/src/lock.rs) line 64, under its doc comment at lines 60-63) —
so leaving a fresh warning would be out of character for the module.

### 5. The exact text below is rustfmt-canonical — measured, not asserted

`crates/cyrup-tools/src/lock.rs` was copied to a scratch file, both edits applied, and
`rustfmt --check --edition 2024` run on the before and after.

**Before** (408 lines) rustfmt reports exactly two diffs, at hunk headers `:89` and `:143`,
covering the offending lines:

```
 92 |         Self { inner: KeyedLocks::new(Arc::clone(&map)), map }
146 |         self.inner.guard(key, cancel).await.map_err(|_| error::aborted())
```

**After** (422 lines; Edit 1 adds 10 lines, Edit 2 adds 4) rustfmt reports **exactly one** diff, the
surviving `FileMutationLocks::new` struct literal, now at hunk header `:99` / offending line 102.
Nothing at the struct, nothing at `guard`.

Two consequences that the edit text below already accounts for:

- The single-line struct declaration measures **97 columns**; every new doc line is ≤ 99. A longer
  `reason` string pushes rustfmt into a multi-line `#[expect(…)]` block, which is why the argument
  lives in the doc comment and `reason` stays terse. Do not lengthen it.
- `guard`'s body must be written pre-split. The chain
  `.inner.guard(key, cancel).await.map(MutationGuard).map_err(…)` exceeds stock `chain_width` (60),
  so rustfmt splits it. Writing it pre-split means this task leaves `guard` fmt-clean.

### 6. No test changes are required, and no `Debug` bound is introduced

The eight tests in `mod tests` (`same_path_serializes`,
`independent_handles_share_one_lock_per_path`, `guard_evicts_its_entry_on_drop`,
`a_cancelled_acquisition_evicts_its_entry_instead_of_leaking_it`,
`dropping_the_acquisition_future_evicts_its_entry`, `missing_path_falls_back_to_the_resolved_path`,
`non_missing_realpath_failure_propagates_instead_of_being_swallowed`,
`distinct_paths_do_not_serialize`) consume the guard only as `let _g = …`, `let g = …`,
`let held = …`, `drop(g)`, and through `Result` combinators. None of those need
`MutationGuard: Debug`:

- `Result::unwrap`/`Result::expect` bound `E: Debug`, not `T` — so `.unwrap()` on
  `Result<MutationGuard, ToolError>` needs only `ToolError: Debug`.
- `Result::err()` has no bounds, and the `Option::expect` that follows it in
  `non_missing_realpath_failure_propagates_instead_of_being_swallowed` has none either.
- `assert!(err.is_err())` has none.
- In `distinct_paths_do_not_serialize`, `.expect(…)` on the `tokio::time::timeout` result bounds
  `Elapsed: Debug`, with `Result<MutationGuard, ToolError>` sitting unbounded in the `T` slot.

`FileMutationLocks::key` still returns `PathBuf`, so the direct map introspection
(`locks.map.contains_key(&key)`) is untouched. The "proven with no test changes at all" property
claimed by the `map` field's comment ([`lock.rs`](../../crates/cyrup-tools/src/lock.rs) lines 60-63)
therefore survives this change. **This task adds no tests and modifies none** — `mod tests` is out
of scope entirely.

## Decision

**Required path: (A) newtype in `cyrup-tools`.** This is the single path to implement; the
alternatives are recorded only so they are not re-litigated.

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

Two find-and-replace edits, both in
[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs). **Nothing else in the
workspace changes** — not `lib.rs`, not `cyrup-core`, not `cyrup-config`, not `mod tests`, not any
`Cargo.toml`.

Both anchors below were matched against the file on disk and each occurs **exactly once**. Confirm
that before editing:

```
$ python3 - <<'EOF'
import io
s = io.open('crates/cyrup-tools/src/lock.rs', encoding='utf-8').read()
a1 = '/// RAII guard for a per-file mutation lock — [`cyrup_core::keyed_lock::KeyedGuard`] keyed by the\n'
a2 = '        let key = Self::key(path).await?;\n        self.inner.guard(key, cancel).await.map_err(|_| error::aborted())\n'
print('anchor1:', s.count(a1))   # must print 1
print('anchor2:', s.count(a2))   # must print 1
EOF
```

If either prints anything other than `1`, stop — the file has drifted and this spec must be
re-derived rather than force-fitted.

### Edit 1 — the guard's doc comment and the `pub type`

**Find this exact text** (5 lines; currently `lock.rs` lines 81-85, ending at the line immediately
before the blank line that precedes `impl FileMutationLocks {`):

```rust
/// RAII guard for a per-file mutation lock — [`cyrup_core::keyed_lock::KeyedGuard`] keyed by the
/// resolved path. On drop it releases the mutex and evicts the map entry once no other
/// holder/waiter references it (Pi deletes the queue entry when it drains,
/// file-mutation-queue.ts:57-59), so the lock map cannot grow without bound.
pub type MutationGuard = KeyedGuard<PathBuf>;
```

**Replace it with this exact text** (15 lines):

```rust
/// RAII guard for a per-file mutation lock — a newtype over
/// [`cyrup_core::keyed_lock::KeyedGuard`] keyed by the resolved path. On drop it releases the
/// mutex and evicts the map entry once no other holder/waiter references it (Pi deletes the queue
/// entry when it drains, file-mutation-queue.ts:57-59), so the lock map cannot grow without bound.
///
/// A newtype and NOT a `pub type` alias, deliberately. `cyrup-config` instantiates the same
/// generic over the same key type for its own, deliberately separate map — its `CONFIG_LOCKS`
/// static and the `FileLock::_in_process` field — so an alias would make a guard proving
/// exclusion over config paths and one proving it over tool-mutated paths literally the same Rust
/// type, and `fn commit(_: MutationGuard)` would accept either. Nothing passes a guard as a value
/// today; the wrapper is what keeps the day one does from type-checking against the wrong domain.
/// It costs nothing: `KeyedGuard` has no public operations to forward, and drop order, drop
/// behaviour and auto-trait membership are exactly the field's. It also re-opens
/// `impl MutationGuard` in this crate, which E0116 forbids on the aliased foreign type.
pub struct MutationGuard(#[expect(dead_code, reason = "held for its Drop")] KeyedGuard<PathBuf>);
```

Three properties of that replacement text are load-bearing and must not be "improved":

- **The field stays private and the wrapper gets no `Drop` impl of its own.** The field's
  `KeyedGuard::drop` is the whole behaviour; an outer `Drop` would only forbid destructuring inside
  this module — and, per §4, would not suppress the `dead_code` warning anyway.
- **`CONFIG_LOCKS` and `FileLock::_in_process` are named, never cited by line number.** Those items
  live in `crates/cyrup-config/src/lock.rs`, which is under active edit by several queued tasks;
  a numeric citation baked into shipped source would go stale. (The previous revision of this spec
  told you to write `cyrup-config/src/lock.rs:19` and `:46` into the comment. Both numbers are
  already wrong — they are 20 and 78 today. Names, not numbers.)
- **`cyrup-tools` does not depend on `cyrup-config`**, so those two names are in plain backticks,
  not `[...]` intra-doc links. `broken_intra_doc_links = "deny"` is set workspace-wide
  ([`Cargo.toml`](../../Cargo.toml) line 107), so linking them would fail the docs build. The one
  bracketed link in the block, `[`cyrup_core::keyed_lock::KeyedGuard`]`, is already present in the
  text being replaced and resolves — `cyrup-core` is a dependency.

### Edit 2 — the body of `FileMutationLocks::guard`

**Find this exact text** (2 lines; currently `lock.rs` lines 145-146, the entire body of `guard`):

```rust
        let key = Self::key(path).await?;
        self.inner.guard(key, cancel).await.map_err(|_| error::aborted())
```

**Replace it with this exact text** (6 lines):

```rust
        let key = Self::key(path).await?;
        self.inner
            .guard(key, cancel)
            .await
            .map(MutationGuard)
            .map_err(|_| error::aborted())
```

`.map` must precede `.map_err`: the success value needs wrapping, the error mapping is unchanged.
Write it broken across lines exactly as shown — that is rustfmt's output for this chain (§5), and
writing it as one line would create a *new* rustfmt violation rather than removing one.

### Not edited

- The `use cyrup_core::keyed_lock::{KeyedGuard, KeyedLockMap, KeyedLocks};` import at `lock.rs`
  line 12 stays exactly as-is. `KeyedGuard` is still named — by the newtype's field — so it does
  not become an unused import.
- `guard`'s own doc comment (`lock.rs` lines 128-139) and its signature (lines 140-144) are
  correct and must not be touched.
- `MutationGuard` is deliberately **not** added to `lib.rs`'s root re-exports. This branch is
  already carrying unintended public-API movement, and `cyrup_tools::lock::MutationGuard` already
  names the type.
- No new tests, no benchmarks, no new documentation files. `mod tests` (`lock.rs` lines 150-408)
  must come out of this task byte-identical.

## Definition of done

Check each item by reading the file and running the listed command. **No git command is used or
needed, and no test is written or run.**

1. **Edit 1 applied verbatim.** `MutationGuard` is a `pub struct` with one private field, no `Drop`
   impl of its own, and the 15-line block above as its declaration:

   ```
   $ grep -c 'pub type MutationGuard' crates/cyrup-tools/src/lock.rs          # 0
   $ grep -c 'pub struct MutationGuard(#\[expect(dead_code, reason = "held for its Drop")\] KeyedGuard<PathBuf>);' crates/cyrup-tools/src/lock.rs   # 1
   $ grep -c 'impl Drop for MutationGuard' crates/cyrup-tools/src/lock.rs     # 0
   ```

2. **Edit 2 applied verbatim.** `guard`'s body is the six-line chain, with `.map(MutationGuard)`
   between `.await` and `.map_err`:

   ```
   $ grep -n -A5 'let key = Self::key(path).await?;' crates/cyrup-tools/src/lock.rs
   ```

   must show `self.inner` / `.guard(key, cancel)` / `.await` / `.map(MutationGuard)` /
   `.map_err(|_| error::aborted())` on five consecutive lines.

3. **Nothing else in the file moved.** The file is 422 lines (was 408: +10 from Edit 1, +4 from
   Edit 2). `FILE_MUTATION_LOCKS`, `is_missing_path_error`, `FileMutationLocks` (both fields,
   `Default`, `new`, `key`), `guard`'s doc comment and signature, and the whole of `mod tests` are
   unchanged. Spot-check the parts most at risk:

   ```
   $ grep -n 'MutationGuard' crates/cyrup-tools/src/lock.rs
   ```

   must list exactly five lines, in this order: the two doc lines mentioning
   `fn commit(_: MutationGuard)` and `impl MutationGuard` (90, 94), the `pub struct` (95), the
   return type in `guard`'s signature (154), and `.map(MutationGuard)` (159).

   `mod tests` begins at `#[cfg(test)]`, line 150 before the edits and line 164 after. Its content
   must be byte-identical across the change — capture the digest before editing and re-run after:

   ```
   $ sed -n '150,408p' crates/cyrup-tools/src/lock.rs | md5sum   # before the edits
   $ sed -n '164,422p' crates/cyrup-tools/src/lock.rs | md5sum   # after — same digest
   ```

   Measured on the scratch application of both edits: both print
   `0f680cff70ad61fe86c378afcb8bf8b1`.

4. **No other file changed.** `crates/cyrup-tools/src/lib.rs`, everything under `crates/cyrup-core`
   and `crates/cyrup-config`, and every `Cargo.toml` are untouched. In particular
   `grep -rn "MutationGuard" --include=*.rs crates/ | grep -v CompletionMutationGuard` still lists
   `crates/cyrup-tools/src/lock.rs` and no other path.

5. **Compiles clean, including the test targets, with no new warning.**

   ```
   $ cargo check -p cyrup-tools --all-targets 2>&1 | grep -E 'never read|unfulfilled_lint_expectation|^error'
   ```

   must print nothing. `--all-targets` type-checks `mod tests` without running it, which is what
   proves §6's "no `Debug` bound is introduced" claim. If `unfulfilled_lint_expectation` fires,
   someone has given the field a reader — **delete the `#[expect(…)]`** rather than widening it to
   `allow`.

6. **rustfmt is not made worse — it is made better by exactly one hunk.**

   ```
   $ rustfmt --check --edition 2024 crates/cyrup-tools/src/lock.rs
   ```

   must report **exactly one** diff: the pre-existing `FileMutationLocks::new` struct literal, hunk
   header `:99`, offending line 102 (`Self { inner: KeyedLocks::new(Arc::clone(&map)), map }`).
   Nothing at the `MutationGuard` declaration and nothing at `guard`. Do **not** run a
   workspace-wide or package-wide `cargo fmt` to "finish the job" — the remaining hunk belongs to
   [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md).

## Consistency with the `KeyedLockMap` finding

[`LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md`](./LOW-keyed-lock-map-alias-exposes-the-raw-dashmap.md)
asks the same question about `pub type KeyedLockMap<K> = Arc<DashMap<K, Arc<Mutex<()>>>>`
([`keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) line 19). The answers agree, and the
shared rule is: **a `pub type` alias is fine when the aliased type carries no invariant of the
aliasing crate, and wrong when the alias is that crate's only expression of one.**

Both aliases fail that rule, but not equally, and the difference should be stated rather than
smoothed over:

- `KeyedLockMap` leaks **capability**. Its target is `DashMap`, whose full mutating surface
  (`insert`, `remove`, `clear`, `alter`) is reachable by every domain owner and can produce two live
  mutexes for one key, voiding exclusion. That is a live hazard in `cyrup-tools` in particular,
  which keeps the raw map as a field (`FileMutationLocks::map`) next to the code that must never
  break atomicity. Its fix is therefore *more* urgent than this one and additionally removes the
  `dashmap` dependency from two crates.
- `MutationGuard` leaks only **identity**. `KeyedGuard`'s fields are private, so no holder can do
  anything but drop it; the only failure mode is a future signature accepting the wrong domain's
  guard. Strictly latent.

So: newtype both. If only one is done, do `KeyedLockMap` first. The two changes compose without
conflict — after both, `FILE_MUTATION_LOCKS` is built with `KeyedLockMap::new()` and
`MutationGuard` still wraps `KeyedGuard<PathBuf>`; neither edit touches the other's lines.

## Interactions

- [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md)
  owns the two pre-existing fmt hunks in this file (offending lines 92 and 146; rustfmt hunk
  headers `:89` and `:143`). **Edit 2 resolves the `:146` one** by writing the canonical form, and
  Edit 1 shifts the surviving `new` hunk from line 92 to line 102. That task's per-file table for
  `crates/cyrup-tools/src/lock.rs` — "2 hunks", and its `+8 / −2` line-delta row — must be updated
  to one hunk at line 102 once this lands. It is scheduled to run last, so it should re-check
  rather than re-format.
- [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md)
  definition-of-done item 7 requires `crates/cyrup-tools/src/lock.rs` to still report **two**
  pre-existing rustfmt diffs. That is only true before this task lands. **Ordering: run that task
  before this one**, or amend its item 7 to expect one hunk at line 102. It edits only the `map`
  field's doc comment (lines 60-63), which neither of this task's anchors touches, so there is no
  textual conflict either way.
- [`LOW-public-api-changes-beyond-the-async-keyword.md`](./LOW-public-api-changes-beyond-the-async-keyword.md)
  lists `MutationGuard` in its "(c) Type changed — breaking" table (the row citing `lock.rs:85`) and
  in its "(c) type changed" count of 3. After this task that row is retired and the count drops to
  2 — update the table rather than leaving a stale entry. Its `lock.rs:85` and `lock.rs:140-146`
  citations are both correct today; the latter becomes `lock.rs:140-160` after Edit 2. Note it also
  refers to the item as `cyrup_tools::MutationGuard` in its downstream-exposure table; the correct
  path is `cyrup_tools::lock::MutationGuard`, which this task does not change.
- [`LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md`](./LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md)
  is the same two-domains theme from the keying side, and is independent: that one is about *how*
  `cyrup-config` derives its keys, this one about whether the resulting guards are distinguishable.
  There is now **no citation coupling at all** — Edit 1's doc comment names `CONFIG_LOCKS` and
  `FileLock::_in_process` and gives no line numbers, so `cyrup-config/src/lock.rs` can move freely.
- [`MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md`](./MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md)
  rewrites the comment on `FileLock::_in_process`. Its own Interactions section warns that this
  task "cites `cyrup-config/src/lock.rs:19` and `:46` by number" — **that is no longer true**, so
  that warning can be struck. The two tasks are fully independent; no conflict in either direction.
