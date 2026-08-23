---
title: Trustpromptfn Public Break And Its Misstated Cost
priority: MEDIUM
stage: qa
status: completed
updated: 2026-08-23 08:27
---

# `TrustPromptFn`: the shape stands, the rustdoc above it does not

Two questions were raised against one declaration. They are answered separately below and only one
of them produces a code change.

| # | Question | Answer | Change |
| --- | --- | --- | --- |
| 1 | Is the new type shape right, or should it be reconsidered before it ships to hosts? | **Right. Keep it.** Both proposed alternatives were compiled and measured against the actual constraint; neither buys what it claims. | none |
| 2 | Does the rustdoc above it describe that shape correctly? | **No.** It asserts a cost that is false and omits the one constraint that determines what an implementor may write. | the doc block immediately above `pub type TrustPromptFn` in [`crates/cyrup-session-svc/src/builder.rs`](../../crates/cyrup-session-svc/src/builder.rs) |

**Required path: the single doc edit in §3. Nothing else.** Do not change the alias, the call site,
the factory, the implementor, or the two stubs.

> This file is the consolidation target of the review that circulated as
> `trustpromptfn-signature-change-breaks-every-host-implementation.md`. That review proposed owned
> arguments to remove the per-invocation clone. **§2 shows by reproduced compiler output that it
> does not remove the clone.** Do not re-propose it.

---

## 0. Citation audit (this pass)

Every pointer in the previous revision was re-checked against the working tree. Result:

| Pointer | Verdict |
| --- | --- |
| `builder.rs:419` field, `:433-437` doc, `:438-445` alias, `:479-482` setter, `:660`, `:677-686`, `:683`, `:684` | all still correct |
| `lib.rs:47-50` re-export | correct |
| `factory.rs:36`, `:66-69`, `:168-170`, `:200-202` | correct |
| `prelaunch.rs:229-247` (`trust_prompt_callback`) | correct — the whole fn is 229-247, the file is 270 lines |
| `prelaunch.rs:236` (the four-way clone) | correct |
| **`prelaunch.rs:1536`** in the old §4 | **STALE — no such line.** It was a `main.rs` number carried over by the decomposition. Replaced below by the name `trust_prompt_callback`. |
| `cyrup-config/src/trust.rs:149/:170/:198` | the lines exist, but the previous text paired them with the names in the wrong order (`:170` is `set_many`, `:198` is `set`). Replaced by names only. |
| `cyrup-tui/src/theme.rs:107`, `cyrup-tui/src/keymap.rs:789` | correct (the `UiTheme` doc line; `pub struct SelectKeymap`) |
| `cyrup/src/startup_ui.rs:386` (`persist_trust_choice`) | correct |
| "the cross-reference in `LOW-public-api-changes-…` still uses the old file name" | **STALE — it does not.** Line 129 of that file already links here by the current name. Claim removed. |
| "`380c713` (2026-08-14) is an ancestor of the merge base" / "at the merge base it was `Arc<dyn Fn(…) -> Option<bool>>`" | **unverifiable from the tree, and not load-bearing.** Removed; §1 now establishes the break from the tree alone. |

Line numbers are retained only where the previous revision used them and they still hold. Where a
thing has a name, this spec cites the **name** — a name cannot rot when a file is re-shuffled.

## 1. The surface, as it actually stands

`pub type TrustPromptFn`, `crates/cyrup-session-svc/src/builder.rs:438-445`:

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

It is re-exported at the crate root, so its shape *is* `cyrup-session-svc`'s public API and any
change to it is a break for every host. That is true of the working tree as it stands and needs no
history to establish.

The whole surface, verified in the current tree:

| Site | Name | Where | What it does |
| --- | --- | --- | --- |
| declaration | `TrustPromptFn` | `cyrup-session-svc/src/builder.rs:438` | the alias |
| re-export | `pub use builder::{… TrustPromptFn}` | `cyrup-session-svc/src/lib.rs:47-50` | crate-root public |
| field | `SessionBuilder::trust_prompt` | `builder.rs:419` | `trust_prompt: Option<TrustPromptFn>` |
| setter | `SessionBuilder::trust_prompt` | `builder.rs:479-482` | `#[must_use]` builder setter |
| field + setter | `SessionFactory::trust_prompt` | `factory.rs:36`, `:66-69` | the factory holds one |
| re-apply | `SessionFactory::build` / `build_from_manager` | `factory.rs:168-170`, `:200-202` | `prompt.clone()` — an `Arc` bump per build, not per prompt |
| **sole invocation** | inside `SessionBuilder::build` | `builder.rs:677-686` | `prompt(&options, &saved).await.unwrap_or(false)` under `TrustOutcome::NeedsPrompt` |
| sole implementor | `cyrup::prelaunch::trust_prompt_callback` | `crates/cyrup/src/prelaunch.rs:229-247` | builds the `Arc` closure |
| bin plumbing | `cyrup::session_launch::build_factory` | `crates/cyrup/src/session_launch.rs:156` | `trust_prompt: Option<TrustPromptFn>` parameter — `Some(…)` for the interactive host only (wired in `run`, `main.rs:450`) |
| stubs | two inline closures | `cyrup-session-svc/src/tests/project_trust_extension.rs:332-335`, `:368-371` | `Box::pin(async { Some(true) })` |

Two facts about the call site matter to §2, both re-verified:

- `options` is a fresh `Vec<TrustOption>` built one line earlier (`trust_options(&cwd, true)`,
  `builder.rs:683`) and is dead after the call.
- `saved` is bound at `builder.rs:630`, read once *before* the prompt into `TrustInputs`
  (`builder.rs:660`), and its last use anywhere in `build()` is the prompt call itself at
  `builder.rs:684`.

## 2. Why the shape is right (question 1, closed)

### The async-ness is not a design choice

`TrustStore::nearest`, `TrustStore::set_many` and `TrustStore::set` are all `pub async fn`
(`crates/cyrup-config/src/trust.rs`). Persisting the chosen option's `updates` is the callback's job
by SEAM-065 (pi runs `saveProjectTrustPromptResult` *inside* `selectProjectTrustOption`), and the
bin's half does exactly that via `persist_trust_choice` (`crates/cyrup/src/startup_ui.rs:386`),
whose only `.await` is `set_many`. A synchronous callback would have to hand the chosen option back
for the builder to persist — a *different* and larger break (the return type stops being
`Option<bool>`), plus a divergence from pi. Async is the smallest change consistent with the branch.

### The two alternatives, compiled

Three shapes were built standalone under `rustc --edition 2024` (single files, no dependencies).
This pass re-ran all three and reproduces the output below. Each was asked the one question that
matters: **can an implementor borrow a captured value into the returned future, instead of cloning
it?**

**A — the current alias.** Rejected:

```
error: lifetime may not live long enough
 |     Arc::new(move |_o, _s| Box::pin(async { Some(!theme.is_empty()) }))
 |              ------------- ^^^ closure was supposed to return data with lifetime `'2`
 |                                 but it is returning data with lifetime `'1`
 = note: closure implements `Fn`, so references to captured variables can't escape the closure
```

**B — owned arguments, no HRTB** (`dyn Fn(Vec<TrustOption>, Option<TrustEntry>) -> Pin<Box<dyn Future + Send>>`),
the fix the earlier review proposed. **Rejected by the same rule:**

```
error: lifetime may not live long enough
 |     Arc::new(move |_o, _s| Box::pin(async { Some(!theme.is_empty()) }))
 |              ------------- ^^^ returning this value requires that `'1` must outlive `'2`
 = note: closure implements `Fn`, so references to captured variables can't escape the closure
```

The desugared receiver is `fn call<'s>(&'s self, args) -> Output`. `'s` is late-bound and
independent of anything in `Output`, so no value the closure returns may borrow through `&self` —
whether the arguments are borrowed or owned, whether the future is `'a` or `'static`. **The clone is
forced by the return type, not by the arguments.** Shape B therefore costs the same clone as shape A
and only trades a tighter contract for a looser one.

**C — a trait whose `'a` covers the receiver.** The only shape that removes the clone; compiles
clean (re-verified this pass):

```rust
pub trait TrustPrompt: Send + Sync {
    fn prompt<'a>(&'a self, options: &'a [TrustOption], saved: &'a Option<TrustEntry>)
        -> Pin<Box<dyn Future<Output = Option<bool>> + Send + 'a>>;
}
// impl body: Box::pin(async move { … self.theme … })   // borrows self, no clone
```

### The call

Shape C is the wrong trade here, and shape B is a downgrade:

- **What the clone actually costs.** `trust_prompt_callback` clones four things per invocation
  (`prelaunch.rs:236`): `UiTheme`, documented as "Cheap to clone (a handful of optional colors + a
  name)" (`crates/cyrup-tui/src/theme.rs:107`); `SelectKeymap`, whose `Default` is a seven-element
  `Vec<(Key, SelectAction)>` (`crates/cyrup-tui/src/keymap.rs:789-808`); a `PathBuf` (`cwd`); and an
  `Arc` bump (the `TrustStore`). That is the price, at most once per session build, immediately
  before blocking on a human at a terminal. Shape C buys back four cheap clones by forcing every
  host to declare a named type and hand-write a lifetime-parameterised method instead of passing a
  closure — a strictly *wider* break than the one being complained about.
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

## 3. The required change — one edit, byte-exact

One file: `/home/user/cyrup/crates/cyrup-session-svc/src/builder.rs`. One replacement. No other file
is touched.

The two defects being fixed:

1. `builder.rs:434` is glued onto the paragraph that ends at `:433` with no `///` separator, so
   rustdoc renders the persistence-responsibility rationale and the boxed-future rationale as one
   run-on block.
2. `builder.rs:436-437` assert `"this costs a boxed allocation per prompt and nothing else"`. §2
   shows that is false: every implementor also clones its captured environment on every invocation,
   and the sole in-tree one does exactly that at `prelaunch.rs:236`. The sentence is also silent on
   the `for<'a>` borrow and on `Send`, so the first host to try spawning the prompt hits an opaque
   lifetime error with nothing at the type to explain it.

### 3.1 FIND — this exact 5-line text (currently `builder.rs:433-437`)

Verified present in the current tree, and verified to occur **exactly once** in the file:

```text
/// rows write nothing.
/// Returns a boxed future because the prompt persists the chosen option through
/// `TrustStore::set_many`, which is async — the host's implementation cannot answer synchronously.
/// The one call site ([`SessionBuilder::build`]) already awaits inside `async fn`, so this costs a
/// boxed allocation per prompt and nothing else.
```

Byte notes for whoever performs the edit:

- The dash in `which is async — the host's` is U+2014 EM DASH (bytes `e2 80 94`), not `--`.
- No line has trailing whitespace; indentation is zero (these are file-level `///` lines).
- Anchoring on `/// rows write nothing.` alone is also unique (1 occurrence), but the full 5-line
  block is the required anchor — replacing fewer lines cannot insert the missing `///` separator.
- Do **not** anchor on `and nothing else`: that substring occurs **twice** in the file (`:437` and
  `:2708`, the latter unrelated, in a `configure_http_proxy` comment).

### 3.2 REPLACE — with this exact 22-line text

```text
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
///   future on each invocation; see `trust_prompt_callback` in `crates/cyrup/src/prelaunch.rs`.
///   Taking the arguments by value would not change this: the `&self` borrow is what is
///   unnameable, not the argument borrows.
/// - `Send` on the future rules out holding a `!Send` UI handle across an await inside the prompt.
///
/// All three are priced for a callback invoked at most once per session build, immediately before
/// blocking on a human at a terminal.
```

All three em dashes in the replacement are U+2014, matching the surrounding block. The widest line
is 99 characters; the surrounding doc block peaks at 100, and the workspace ships no `rustfmt.toml`,
so `max_width = 100` is the governing default. Nothing in the replacement exceeds it.

### 3.3 Expected post-edit geometry

- 5 lines out, 22 lines in ⇒ **+17 lines**.
- The rewritten doc block occupies `builder.rs:433-454`.
- `pub type TrustPromptFn = Arc<` moves from `:438` to **`:455`**; the alias body ends at `:462`.
- Every subsequent line in `builder.rs` shifts by exactly +17. No code line's *text* changes.
- The intra-doc link `[`SessionBuilder::build`]` is carried over unmodified from the existing text
  and still resolves: `pub async fn build` is at `builder.rs:605` before the edit, `:622` after.

## 4. Explicitly out of scope

- Changing `TrustPromptFn` to owned arguments (shape B). **Closed by §2 — it removes no clone.**
- Introducing a `TrustPrompt` trait (shape C). Closed by §2 — a wider break than the one it fixes.
- Reverting the callback to synchronous. Closed by §2 — `TrustStore` is async and the persist is the
  callback's job by SEAM-065.
- Touching `trust_prompt_callback` (`crates/cyrup/src/prelaunch.rs:229-247`). The four-way clone on
  the closure's first line (`:236`) is the correct and only way to satisfy the bound; this task
  documents it rather than removing it.
- Touching the two stubs in `crates/cyrup-session-svc/src/tests/project_trust_extension.rs`
  (`:332-335`, `:368-371`). `Box::pin(async { Some(true) })` is the minimal conforming stub.
- Adding, renaming or re-running tests, benchmarks, or doc examples. Nothing behavioural changes and
  another team owns test work.
- Authoring any new document — changelog, migration note, or book page. The workspace has no
  `CHANGELOG`, the crate is version `0.0.0` with no registry release, and the rewritten rustdoc *is*
  the guidance a host hitting the compile error needs.
- Editing `LOW-public-api-changes-beyond-the-async-keyword.md`. Note for a later pass only: its line
  129 cites `builder.rs:438-445` for this alias, which this edit shifts to `:455-462`. That is a
  different task's file and must not be touched here.

## 5. Definition of done

No tests are written or run, and no git command is used. Each check below is a direct read of the
file on disk.

1. **The FIND text is gone.** `grep -c 'boxed allocation per prompt and nothing else'
   crates/cyrup-session-svc/src/builder.rs` prints `0`.
2. **The REPLACE text is present, exactly once.** Reading `builder.rs:433-454` reproduces the 22
   lines of §3.2 character for character, including both bare `///` separator lines and the U+2014
   em dashes.
3. **The run-on paragraph is broken.** `builder.rs:433` is `/// rows write nothing.` and
   `builder.rs:434` is a bare `///`.
4. **The alias is untouched and has moved by exactly +17.** `grep -n 'pub type TrustPromptFn'
   crates/cyrup-session-svc/src/builder.rs` prints `455:pub type TrustPromptFn = Arc<`, and the six
   lines that follow it are byte-identical to the block quoted in §1.
5. **All three implementor constraints are stated** in the new doc: the `'a` borrow of both
   arguments, the unnameable `&self` borrow that forces the per-invocation clone, and `Send` on the
   future.
6. **Shape B is pre-empted in prose,** so the next reader does not re-derive and re-propose it: the
   doc says in as many words that taking the arguments by value would not change this.
7. **Nothing but comment lines changed.** In `builder.rs`, the only difference from the starting
   state is the block at `:433-454`, and every line of it begins with `///`. No other file in the
   workspace differs. Verify by reading, not by `git diff`.
8. **Widths hold.** No line in `builder.rs:433-454` exceeds 100 characters (longest is 99).
9. **It still compiles.** `cargo check -p cyrup-session-svc` succeeds. Doc-only change; this is a
   guard against a mangled `///` prefix or a stray character, nothing more.

---

## 6. QA verdict (2026-08-23 08:27) — PASS, 9/10

Review-only pass. No source file touched. No git command used.

| DoD | Result |
| --- | --- |
| 1. FIND text gone | `grep -c 'boxed allocation per prompt and nothing else'` ⇒ `0` |
| 2. REPLACE present once | `builder.rs:433-454` diffs byte-identical against §3.2 (22 lines, 3× U+2014, no trailing whitespace) |
| 3. Run-on broken | `:433` = `/// rows write nothing.`, `:434` = bare `///` |
| 4. Alias untouched, +17 | `455:pub type TrustPromptFn = Arc<`; `:455-462` diffs byte-identical against §1 |
| 5. Three constraints stated | `'a` borrow of both args, unnameable `&self`, `Send` — all present |
| 6. Shape B pre-empted | "Taking the arguments by value would not change this" (`:449-450`) |
| 7. Comment lines only | all 22 lines begin with `///`; alias body byte-identical to its pre-edit quote |
| 8. Widths | max 99 chars (`:435`, `:451`); `:436` is 100 *bytes* / 98 chars (one em dash) |
| 9. Compiles | `cargo check -p cyrup-session-svc --offline` ⇒ `Finished` in 39.11s, exit 0 |

Truth audit of every new assertion (not taken on the doc's word):

- `TrustStore::set_many` is `pub async fn` (`cyrup-config/src/trust.rs:170`) and is what
  `persist_trust_choice` awaits (`crates/cyrup/src/startup_ui.rs:391-393`). ✔
- Sole invocation is `prompt(&options, &saved).await` inside `pub async fn build` (`:622`), in the
  `TrustOutcome::NeedsPrompt` arm at `:694-701`, unlooped — "at most once per session build". ✔
- The `&self`/clone claim was re-derived from `rustc --edition 2024`, not trusted: shape A and
  shape B both fail with *closure implements `Fn`, so references to captured variables can't
  escape the closure*; the clone variant of shape A compiles. Both the constraint and the
  "by-value would not change this" pre-emption are true. ✔
- The sole implementor does clone per invocation: `let (theme, keymap, cwd, store) = (theme.clone(),
  keymap.clone(), cwd.clone(), store.clone());` on the closure's first line, `prelaunch.rs`
  `trust_prompt_callback`. Cited by name, so it cannot rot. ✔
- The old sentence's falsehood ("a boxed allocation per prompt and nothing else") is gone and its
  replacement scopes the box to what the *builder* pays. ✔

Nits, none blocking:

- "the box is the only per-prompt cost the *builder* pays" ignores the fresh `trust_options(&cwd,
  true)` `Vec` built one line before the call — that `Vec` is required by the signature either way,
  so the sentence is fair in context.
- "the returned future borrows `options` and `saved` for `'a`" is really "is bounded by `'a` and
  may borrow them"; the stated consequence (cannot be spawned, stored, or outlive the call) holds
  regardless.
- "read off the signature above" is true of the rustdoc rendering (signature first), not of the
  source order.
- The +17 shift dates in-source citations of `builder.rs` line numbers elsewhere in the tree
  (`session_bind.rs:114`, `blocking.rs:602`, …). Checked: those were **already** stale by other
  amounts before this edit (e.g. `builder.rs:597` for `has_trust_requiring_resources`, actually at
  `:636` now / `:619` pre-edit), so this change introduces no new class of drift. §4 scoped it out.
- §0 of this spec lists `startup_ui.rs:386` for `persist_trust_choice`; the `async fn` is at `:387`.
  Spec-internal, not shipped in any comment.
