---
title: Trustpromptfn Public Break And Its Misstated Cost
priority: MEDIUM
stage: aug
status: done
updated: 2026-08-23 03:35
---

# `TrustPromptFn`: the shape stands, the rustdoc above it does not

Two questions were raised against one declaration. They are answered separately below and only one
of them produces a code change.

| # | Question | Answer | Change |
| --- | --- | --- | --- |
| 1 | Is the new type shape right, or should it be reconsidered before it ships to hosts? | **Right. Keep it.** Both proposed alternatives were compiled and measured against the actual constraint; neither buys what it claims. | none |
| 2 | Does the rustdoc above it describe that shape correctly? | **No.** It asserts a cost that is false and omits the one constraint that determines what an implementor may write. | [`builder.rs:433-437`](../../crates/cyrup-session-svc/src/builder.rs) |

**Required path: the doc fix in §3 only.** Do not change the alias, the call site, the factory, the
implementor, or the two stubs.

> This file is the consolidation target of the review that circulated as
> `trustpromptfn-signature-change-breaks-every-host-implementation.md`; the cross-reference under
> "(c) Type changed" in
> [`LOW-public-api-changes-beyond-the-async-keyword.md`](LOW-public-api-changes-beyond-the-async-keyword.md)
> still uses that old name and resolves here. That review proposed owned arguments to remove the
> per-invocation clone. **§2 shows by compiler output that it does not remove the clone.** Do not
> re-propose it.

---

## 1. The surface, as it actually stands

`pub type TrustPromptFn`, [`crates/cyrup-session-svc/src/builder.rs:438-445`](../../crates/cyrup-session-svc/src/builder.rs):

```rust
pub type TrustPromptFn = Arc<
    dyn for<'a> Fn(
            &'a [TrustOption],
            &'a Option<TrustEntry>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<bool>> + Send + 'a>>
        + Send
        + Sync,
>;
```

At the merge base it was `Arc<dyn Fn(&[TrustOption], &Option<TrustEntry>) -> Option<bool> + Send + Sync>`.
It is the only `pub type` in the diff whose *type* changed, and it is a real break of something on
`main`: the alias landed in `380c713` (2026-08-14), which is an ancestor of the merge base.

The whole surface, verified:

| Site | File | What it does |
| --- | --- | --- |
| declaration | [`builder.rs:438`](../../crates/cyrup-session-svc/src/builder.rs) | the alias |
| re-export | [`lib.rs:47-50`](../../crates/cyrup-session-svc/src/lib.rs) | `pub use builder::{… TrustPromptFn}` — crate-root public |
| field | [`builder.rs:419`](../../crates/cyrup-session-svc/src/builder.rs) | `trust_prompt: Option<TrustPromptFn>` |
| setter | [`builder.rs:479-482`](../../crates/cyrup-session-svc/src/builder.rs) | `SessionBuilder::trust_prompt` |
| field + setter | [`factory.rs:36`](../../crates/cyrup-session-svc/src/factory.rs), [`:66-69`](../../crates/cyrup-session-svc/src/factory.rs) | `SessionFactory` holds one and re-applies it |
| re-apply | [`factory.rs:168-170`](../../crates/cyrup-session-svc/src/factory.rs), [`:200-202`](../../crates/cyrup-session-svc/src/factory.rs) | `prompt.clone()` — an `Arc` bump per build, not per prompt |
| **sole invocation** | [`builder.rs:677-686`](../../crates/cyrup-session-svc/src/builder.rs) | `prompt(&options, &saved).await.unwrap_or(false)` under `TrustOutcome::NeedsPrompt` |
| sole implementor | [`prelaunch.rs:229-247`](../../crates/cyrup/src/prelaunch.rs) | `trust_prompt_callback` |
| stubs | [`tests/project_trust_extension.rs:332-335`](../../crates/cyrup-session-svc/src/tests/project_trust_extension.rs), [`:368-371`](../../crates/cyrup-session-svc/src/tests/project_trust_extension.rs) | `Box::pin(async { Some(true) })` |

Two facts about the call site matter to §2:

- `options` is a fresh `Vec` built one line earlier (`trust_options(&cwd, true)`, `builder.rs:683`)
  and is dead after the call.
- `saved` is read once *before* the prompt (`builder.rs:660`, into `TrustInputs`) and is dead after
  `builder.rs:684`. Nothing in `build()` touches it again.

## 2. Why the shape is right (question 1, closed)

### The async-ness is not a design choice

`TrustStore` went async in this same branch — `nearest`, `set`, and `set_many` are all `pub async fn`
now ([`cyrup-config/src/trust.rs:149`, `:170`, `:198`](../../crates/cyrup-config/src/trust.rs)).
Persisting the chosen option's `updates` is the callback's job by SEAM-065 (pi runs
`saveProjectTrustPromptResult` *inside* `selectProjectTrustOption`), and the bin's half does exactly
that via [`persist_trust_choice`](../../crates/cyrup/src/startup_ui.rs) (`startup_ui.rs:386`), whose
only await is `set_many`. A synchronous callback would have to hand the chosen option back for the
builder to persist — a *different* and larger break (the return type stops being `Option<bool>`),
plus a divergence from pi. Async is the smallest change consistent with the branch.

### The two alternatives, compiled

Three shapes were built standalone under `rustc --edition 2024`, each asked the one question that
matters: **can an implementor borrow a captured value into the returned future, instead of cloning
it?**

**A — the current alias.** Rejected:

```
error: lifetime may not live long enough
   |     Arc::new(move |_o, _s| Box::pin(async { Some(theme.len() > 0) }))
   |              ------------- ^^^ closure was supposed to return data with lifetime `'2`
   |                                 but it is returning data with lifetime `'1`
   = note: closure implements `Fn`, so references to captured variables can't escape the closure
```

**B — owned arguments, no HRTB** (`dyn Fn(Vec<TrustOption>, Option<TrustEntry>) -> Pin<Box<dyn Future + Send>>`),
the fix the earlier review proposed. **Rejected by the same rule:**

```
error: lifetime may not live long enough
   |     Arc::new(move |_o, _s| Box::pin(async { Some(theme.len() > 0) }))
   |              ------------- ^^^ returning this value requires that `'1` must outlive `'2`
   = note: closure implements `Fn`, so references to captured variables can't escape the closure
```

The desugared receiver is `fn call<'s>(&'s self, args) -> Output`. `'s` is late-bound and
independent of anything in `Output`, so no value the closure returns may borrow through `&self` —
whether the arguments are borrowed or owned, whether the future is `'a` or `'static`. **The clone is
forced by the return type, not by the arguments.** Shape B therefore costs the same clone as shape A
and only trades a tighter contract for a looser one.

**C — a trait whose `'a` covers the receiver.** The only shape that removes the clone; compiles
clean:

```rust
pub trait TrustPrompt: Send + Sync {
    fn prompt<'a>(&'a self, options: &'a [TrustOption], saved: &'a Option<TrustEntry>)
        -> Pin<Box<dyn Future<Output = Option<bool>> + Send + 'a>>;
}
// impl body: Box::pin(async move { … self.theme … })   // borrows self, no clone
```

### The call

Shape C is the wrong trade here, and shape B is a downgrade:

- **What the clone actually costs.** `UiTheme` is documented as "cheap to clone (a handful of
  optional colors + a name)" ([`cyrup-tui/src/theme.rs:107`](../../crates/cyrup-tui/src/theme.rs));
  `SelectKeymap` is a seven-element `Vec<(Key, SelectAction)>`
  ([`keymap.rs:789`](../../crates/cyrup-tui/src/keymap.rs)); the other two are a `PathBuf` and an
  `Arc` bump. That is the price, at most once per session build, immediately before blocking on a
  human at a terminal. Shape C buys back four cheap clones by forcing every host to declare a named
  type and hand-write a lifetime-parameterised method instead of passing a closure — a strictly
  *wider* break than the one being complained about.
- **Shape B's `'static` future is a weaker contract, not a better one.** Its selling point is that a
  host may spawn, store, or `select!` on the returned future. Nothing can use that: `build()` must
  have the answer before it can compute `trusted` (`builder.rs:684`), so a detached prompt is
  meaningless — and for a *security* decision, "you must answer within this call" is worth having as
  a type-level fact. Shape A's `+ 'a` is what states it.
- **Shape A costs the call site nothing.** `options` is already a fresh `Vec` and `saved` is dead
  after the call, so shape B would not even have cost a clone there — which is the whole of what it
  had to offer, and it is worth nothing.
- **`Send` on the future** is free in-tree (the TUI implementor and both stubs satisfy it) and keeps
  `build()`'s own future `Send`. It is a genuine constraint on a host holding a `!Send` UI handle
  across an await, which is precisely why §3 makes the rustdoc say so.

**Conclusion: the alias is correct as written. No redesign. The remaining defect is the doc.**

## 3. The required change

One hunk, [`crates/cyrup-session-svc/src/builder.rs:433-437`](../../crates/cyrup-session-svc/src/builder.rs).

Two defects there:

1. Line 434 is glued onto the paragraph that ends at line 433 with no `///` separator, so rustdoc
   renders the persistence-responsibility rationale and the boxed-future rationale as one run-on
   block.
2. Lines 436-437 assert `"this costs a boxed allocation per prompt and nothing else"`. §2 shows that
   is false: every implementor also clones its captured environment on every invocation, and the
   sole in-tree one does exactly that at [`prelaunch.rs:236`](../../crates/cyrup/src/prelaunch.rs). The
   sentence is also silent on the `for<'a>` borrow and on `Send`, so the first host to try spawning
   the prompt hits an opaque lifetime error with nothing at the type to explain it.

Replace lines 433-437 — from `/// rows write nothing.` through `/// boxed allocation per prompt and nothing else.` — with:

```rust
/// rows write nothing.
///
/// Returns a boxed future because that persist runs through `TrustStore::set_many`, which is async
/// (`cyrup-config/src/trust.rs`) — the host's implementation cannot answer synchronously. The one
/// call site ([`SessionBuilder::build`]) already awaits inside `async fn`, so the box is the only
/// per-prompt cost the *builder* pays.
///
/// Implementors face three constraints, all of them read off the signature above:
///
/// - The returned future borrows `options` and `saved` for `'a`, so it cannot be spawned, stored,
///   or outlive the call. That is deliberate: `build()` cannot settle the trust flag until the
///   prompt answers, so a detached prompt would be meaningless.
/// - `'a` is the *arguments'* lifetime. It cannot name the `Fn`'s own `&self` borrow — that
///   lifetime is elided, separate, and absent from the return type — so nothing the closure
///   captured may be borrowed into the future. Every implementor clones what it needs into the
///   future on each invocation; see `trust_prompt_callback` in `crates/cyrup/src/prelaunch.rs`. Taking
///   the arguments by value would not change this: the `&self` borrow is what is unnameable, not
///   the argument borrows.
/// - `Send` on the future rules out holding a `!Send` UI handle across an await inside the prompt.
///
/// All three are priced for a callback invoked at most once per session build, immediately before
/// blocking on a human at a terminal.
```

Keep lines at or under 100 columns, matching the surrounding block. Nothing else in the file moves —
in particular the `pub type` at `:438-445` is unchanged, so the line numbers of everything below
shift by exactly the count of lines added here.

## 4. Explicitly out of scope

- Changing `TrustPromptFn` to owned arguments (shape B). **Closed by §2 — it removes no clone.**
- Introducing a `TrustPrompt` trait (shape C). Closed by §2 — a wider break than the one it fixes.
- Reverting the callback to synchronous. Closed by §2 — `TrustStore` is async in this branch and the
  persist is the callback's job by SEAM-065.
- Touching [`prelaunch.rs:229-247`](../../crates/cyrup/src/prelaunch.rs). The four-way clone at `:1536` is
  the correct and only way to satisfy the bound; it is now documented rather than removed.
- Touching the two stubs at
  [`tests/project_trust_extension.rs:332`](../../crates/cyrup-session-svc/src/tests/project_trust_extension.rs)
  and `:368`. `Box::pin(async { Some(true) })` is the minimal conforming stub.
- Adding tests. Nothing behavioural changes.
- A migration note or changelog entry: the workspace has no `CHANGELOG`, the crate is version
  `0.0.0` with no registry release, and the rewritten rustdoc *is* the guidance a host hitting the
  compile error needs — it states exactly what the new implementation must look like.

## 5. Definition of done

- [ ] `crates/cyrup-session-svc/src/builder.rs` line 433 is followed by a bare `///` before the
      boxed-future paragraph, so rustdoc renders two paragraphs and not one.
- [ ] The string `"and nothing else"` no longer appears above `TrustPromptFn`.
- [ ] The doc states all three implementor constraints: the `'a` borrow of both arguments, the
      unnameable `&self` borrow that forces the per-invocation clone, and `Send` on the future.
- [ ] The doc explicitly says that owned arguments would *not* remove the clone, so the next reader
      does not re-derive and re-propose shape B.
- [ ] `git diff --stat` for this task shows `crates/cyrup-session-svc/src/builder.rs` and nothing
      else, with zero non-comment lines changed.
- [ ] `cargo doc -p cyrup-session-svc` renders the two paragraphs and the bullet list, and the
      intra-doc link `[`SessionBuilder::build`]` still resolves.
