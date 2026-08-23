---
title: With Lock Let Next Comments Invent A Borrowck Rule
priority: MEDIUM
stage: aug
status: done
updated: 2026-08-23 03:36
---

# Both `with_lock` impls were restructured — and commented — to satisfy a borrow-check rule that does not exist

## Problem

Converting `SettingsStore::with_lock` to `async`, the branch split `if let Some(x) = f(...)` into
`let next = f(...); if let Some(x) = next` in **both** impls of
[`crates/cyrup-config/src/settings/store.rs`](../../crates/cyrup-config/src/settings/store.rs), and
attached a comment to each asserting that the `if let` form would not borrow-check.

`store.rs:80-85` — `FileSettingsStore::with_lock`:

```rust
        // Bound before the write: inside an `async` body the `if let` scrutinee's borrow of
        // `current` would otherwise still be live across the call below.
        let next = f(current.as_deref());
        if let Some(new_text) = next {
            crate::lock::write_atomic(&path, new_text.as_bytes(), false)?;
        }
```

`store.rs:131-136` — `InMemorySettingsStore::with_lock`:

```rust
        // Same reason as above: bind first so the immutable borrow of `guard` ends before the
        // assignment takes it mutably.
        let next = f(guard.as_deref());
        if let Some(new) = next {
            *guard = Some(new);
        }
```

Neither claim is true. The restructure is **inert**: it neither fixes nor causes anything. The
entire fix was the explicit `for<'s>` HRTB on `f`. The second comment is additionally wrong on its
face — `InMemorySettingsStore::with_lock` (`:122-138`) contains no `.await` at all, so "same reason
as above: inside an `async` body" describes a body with no suspension point in it.

---

## Research — what was verified

### 1. The 2×2 matrix — the HRTB is the whole fix, the restructure is orthogonal

An isolated crate reproduces the real shape faithfully: `#[async_trait]` trait plus both impls, a
`FileLock` with a real `impl Drop`, a genuine `tokio::task::spawn_blocking(..).await` inside
`acquire` before the closure is called, the `std::sync::MutexGuard` in the in-memory impl, and a
`SettingsManager::set`-shaped caller passing `&mut |current| { corrupt = …; }` behind
`Arc<dyn SettingsStore>`. Two axes were varied independently — HRTB present/absent, `let next` /
`if let` — over rustc 1.98.0, async-trait 0.1.89, edition 2024 (identical results on 2021):

| | `if let Some(x) = f(..)` | `let next = f(..); if let Some(x) = next` |
| --- | --- | --- |
| `dyn for<'s> FnMut(Option<&'s str>) -> Option<String> + Send` | **compiles** | **compiles** |
| `dyn FnMut(Option<&str>) -> Option<String> + Send` (merge-base elision) | fails | fails |

The two failing cells produce **byte-identical error sets** (diffed): `E0597` on `current`, `E0597`
on `guard`, `E0502` on `guard`, plus one `lifetime may not live long enough`. The restructure does
not change the diagnostics, let alone the outcome.

That settles both directions at once: the `let next` binding is not *sufficient* (the bottom-right
cell fails with it) and not *necessary* (the top-left cell passes without it). Only the column
heading matters; only the HRTB row does.

Representative failure, showing that the blame lands on the *signature*, never on the statement
shape:

```
error[E0502]: cannot borrow `guard` as mutable because it is also borrowed as immutable
94 |         f: &mut (dyn FnMut(Option<&str>) -> Option<String> + Send),
   |                                   - lifetime `'life2` defined here
97 |         if let Some(new) = f(guard.as_deref()) {
   |                            argument requires that `guard` is borrowed for `'life2`
98 |             *guard = Some(new);
   |              ^^^^^ mutable borrow occurs here
```

`'life2` is `async_trait`'s invention. Nothing in the body can satisfy a lifetime the caller picks.

### 2. Why the HRTB is needed — this reasoning is correct and must survive the cleanup

Read from async-trait 0.1.89 (`src/lifetime.rs`, `src/expand.rs`, the version pinned in
[`Cargo.lock`](../../Cargo.lock)):

- `CollectLifetimes::visit_type_reference_mut` (`lifetime.rs:54-57`) calls `visit_opt_lifetime` on
  **every** `&` it reaches and then recurses via `visit_mut::visit_type_reference_mut`. Recursion
  runs through `Type::Path` generic arguments and `Type::TraitObject` → `TraitBound` →
  `PathArguments::Parenthesized`, so the `&str` nested inside `Option<&str>` inside the `Fn(..)`
  sugar is reached like any other.
- An elided `&` is *renamed*: `next_lifetime` (`lifetime.rs:36-42`) mints `'life0`, `'life1`,
  `'life2`, … and `expand.rs:238-243` pushes each one onto `sig.generics.params` with a
  `'lifeN: 'async_trait` predicate. That makes it an **early-bound method generic chosen by the
  caller** — precisely what the errors above report.
- An explicit lifetime is *not* renamed: `visit_lifetime` (`lifetime.rs:29-35`) only records it in
  `explicit`, and `expand.rs:223` filters that list against the trait/impl's own generic params via
  `Context::lifetimes` (`expand.rs:40-53`). `'s` is bound by the `for<'s>` binder, not by the trait,
  so it matches nothing and is left completely alone.

So writing `for<'s>` restores exactly the merge-base meaning — Fn-sugar elision already produced an
implicit `for<'a>` there — and nothing wider or narrower. It is the right fix, not a workaround.

### 3. `+ Send` is load-bearing too, and excludes no caller

Same crate, HRTB present, `+ Send` removed → `error: future cannot be sent between threads safely`,
twice. `async_trait` boxes the body as `Pin<Box<dyn Future + Send>>` and `&mut T: Send` holds only
when `T: Send`. The only four call sites are in
[`manager.rs`](../../crates/cyrup-config/src/settings/manager.rs) (`:232`, `:292`, `:338`, `:426`),
each passing a closure capturing `&mut Option<String>` plus shared refs — all `Send` — and the only
two impls of the trait are the two in `store.rs`.

### 4. The `InMemory` comment is false clause by clause

- *"Same reason as above: inside an `async` body"* — `InMemorySettingsStore::with_lock`
  (`:122-138`) has no `.await`. There is no suspension point for the borrow to cross.
- *"so the immutable borrow of `guard` ends before the assignment takes it mutably"* — with the
  HRTB present, NLL already ends the shared borrow at the call: `f` returns an owned
  `Option<String>`, and the intermediate `Option<&str>` has no destructor. Verified: the top-left
  cell of the matrix is exactly this body with `if let Some(new) = f(guard.as_deref())`.
- It also points a future reader at the wrong conclusion on the one question this impl invites —
  whether a `std::sync::MutexGuard` is held across an await. It is not: no await, the future
  completes in one poll, and `async_trait`'s `+ Send` bound compiles, which proves it.

### 5. No behavioural difference either way

`f` is called exactly once in both spellings and the matched value is the same owned
`Option<String>`. The intermediate `Option<&str>` has no destructor, so even the temporary-scope
difference is unobservable — including under edition 2024's `if let` rescoping, which if anything
shortens the scrutinee temporary's life relative to the merge base. The revert is a pure
diff-and-comment cleanup with zero runtime effect.

---

## Required change — one file, three hunks

[`crates/cyrup-config/src/settings/store.rs`](../../crates/cyrup-config/src/settings/store.rs).
Nothing else. **Revert both bodies to the merge-base `if let` form, delete both comments, and put
the one true note on the signature that actually had to change.**

### Hunk 1 — the trait method rustdoc (`:18-24`)

The declaration itself does not change. Extend its doc comment so the only non-obvious tokens in
the signature are explained once, where they are declared:

```rust
    /// Serialized read-modify-write. `f` receives the current text (None if absent) and returns
    /// `Some(new)` to write or `None` to leave untouched.
    ///
    /// `for<'s>` is spelled out because `#[async_trait]` rewrites every elided `&` in the signature
    /// into a method-level named lifetime, and `CollectLifetimes::visit_type_reference_mut`
    /// recurses into the `Fn(..)` sugar as well: a plain `FnMut(Option<&str>)` here loses the
    /// implicit `for<'a>` that Fn-sugar elision would give it and becomes early-bound to the
    /// caller. Both impls hand `f` a borrow of a local (`current`, `guard`), which an early-bound
    /// lifetime cannot accept. `+ Send` is load-bearing too: the body is boxed as
    /// `Pin<Box<dyn Future + Send>>`, and `&mut T: Send` only when `T: Send`.
    async fn with_lock(
        &self,
        scope: SettingsScope,
        f: &mut (dyn for<'s> FnMut(Option<&'s str>) -> Option<String> + Send),
    ) -> Result<(), ConfigError>;
```

### Hunk 2 — `FileSettingsStore::with_lock` (`:80-85`)

Delete the two comment lines and the `next` binding; restore the merge-base statement verbatim:

```rust
        if let Some(new_text) = f(current.as_deref()) {
            crate::lock::write_atomic(&path, new_text.as_bytes(), false)?;
        }
```

Everything above it in the body — `let _guard = crate::lock::FileLock::acquire(&path, None).await?;`
and the `match std::fs::read_to_string(&path)` — is unchanged.

### Hunk 3 — `InMemorySettingsStore::with_lock` (`:131-136`)

Same:

```rust
        if let Some(new) = f(guard.as_deref()) {
            *guard = Some(new);
        }
```

After these three hunks the two impl bodies are **byte-identical to merge base
`4902cddf8ce7d4723e41b4a7bf652361a584f905`** apart from the `FileLock::acquire` line, which is a
different task's change. `git diff` on this file then shows only what the async conversion actually
required: the two `#[async_trait]` attributes, three `async fn` keywords, three `f` parameter types,
the awaited `acquire`, and doc comments.

---

## Paths considered and rejected

- **Keep `let next`, just correct the comments.** The binding exists *only* to serve the false
  claim; with the claim gone it has no author and no justification, and the diff still shows a body
  change in an impl that was supposed to change only its signature. Worse, a corrected comment
  ("this binding is not required") is a comment about nothing.
- **Keep `let next` for readability or step-debugging.** `f` is called once either way and `next`
  is consumed on the following line, so the name carries no information the pattern does not. The
  surrounding code (`match std::fs::read_to_string`, `self.slot(scope).lock()`) does not spread
  single-use temporaries either.
- **Put the HRTB note on the two impl signatures instead of the trait.** Three copies of one rule is
  the exact failure mode this task removes. The impls restate a declared signature; the declaration
  is where the rule lives.
- **Delete every comment, including the HRTB rationale.** Rejected: `for<'s>` is the one token in
  this file that a future contributor would confidently "simplify" back to `FnMut(Option<&str>)`,
  and §1 shows that does not compile. The rule is non-obvious, verified, and cheap to state.
- **Change the signature instead — take `Option<String>` by value, or a named generic lifetime.**
  Out of scope and worse: by-value forces a clone per call, and a named early-bound lifetime is the
  exact thing that fails.
- **`#[allow(...)]` / `#[rustfmt::skip]` anywhere.** Nothing here is being suppressed; the code
  compiles clean once the HRTB is present.

## Do not touch

- [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs) — `FileLock`,
  `acquire`'s `cancel` parameter, and its `Drop` are all other tasks' territory.
- The four call sites in
  [`manager.rs`](../../crates/cyrup-config/src/settings/manager.rs) (`:232`, `:292`, `:338`,
  `:426`) — unchanged, including their closures.
- The `#[async_trait::async_trait]` attributes, the `async fn` keywords, and the `f` parameter type
  including `for<'s>` and `+ Send`. This task removes an unnecessary body change; it does not
  revisit the conversion.
- The trait-level rustdoc paragraph at `:9-12` (`#[async_trait]` rather than a native `async fn`…).
  The "why did `read` stay sync" sentence belongs to
  [`LOW-public-api-changes-beyond-the-async-keyword.md`](./LOW-public-api-changes-beyond-the-async-keyword.md),
  not here.
- Tests. No test asserts on statement shape and none needs to change; the behaviour is provably
  identical (§5).
- No workspace-wide `cargo fmt`. `store.rs` is already rustfmt-clean
  (per [`LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md`](./LOW-branch-leaves-rustfmt-violations-in-its-own-new-lines.md))
  and these hunks keep it that way — the restored lines are the merge-base bytes and every new doc
  line is ≤ 100 columns.

## Definition of done

1. `crates/cyrup-config/src/settings/store.rs` is the only file changed by this task.
2. `grep -n 'let next' crates/cyrup-config/src/settings/store.rs` returns nothing. Both `with_lock`
   bodies use `if let Some(..) = f(..)` directly.
3. Neither impl body contains any comment. In particular no comment in this file mentions `async`
   bodies, `if let` scrutinees, or borrow ordering.
4. `for<'s>` appears in exactly three signatures (`:20-24` trait, `:63-67` file impl, `:122-126`
   in-memory impl, pre-edit numbering) and is explained in exactly one place — the trait method's
   rustdoc, per hunk 1.
5. `git diff 4902cddf8ce7d4723e41b4a7bf652361a584f905 -- crates/cyrup-config/src/settings/store.rs`
   shows no `+`/`-` line inside either `with_lock` body other than the `FileLock::acquire` line.
6. `cargo check -p cyrup-config` compiles and `rustfmt --check --edition 2024
   crates/cyrup-config/src/settings/store.rs` is silent.

## Not in this task

- Whether `read` should also become `async` —
  [`LOW-public-api-changes-beyond-the-async-keyword.md`](./LOW-public-api-changes-beyond-the-async-keyword.md).
- The `cancel: Option<&CancelToken>` parameter on `FileLock::acquire` and its cancellation
  behaviour.
- Any changelog / migration note for the `SettingsStore` breaking change.
