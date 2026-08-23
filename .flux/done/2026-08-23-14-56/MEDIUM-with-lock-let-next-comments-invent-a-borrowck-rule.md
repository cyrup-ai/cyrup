---
title: With Lock Let Next Comments Invent A Borrowck Rule
priority: MEDIUM
stage: done
status: done
updated: 2026-08-23 (closed out)
---

# One surviving copy of the invented borrow-check rule — and it now cites the note that was deleted

The `store.rs` half of this task is **done and verified correct** (see "Settled" below — do not redo
any of it, and do not touch `crates/cyrup-config/src/settings/store.rs` again).

What is outstanding is a **third copy** of the same invented rule, in a different crate, which
research §3 of the original task missed because its `with_lock` grep was reported as finding nothing
outside `cyrup-config`. It does not just repeat the falsehood — it *cites the deleted note by
file:line*, so the deletion turned it into a dangling reference on top of a false one.

---

## Outstanding item 1 — delete the dangling false comment in `subcommands.rs`

File: [`crates/cyrup/src/subcommands.rs`](../../crates/cyrup/src/subcommands.rs), inside the async
`on_apply` closure passed to `run_startup_selector` in `async fn run_config` (currently lines
894-895; cite by item name, the line numbers drift).

Present today, verbatim:

```rust
        // Bound before the `if let`, matching `FileSettingsStore::with_lock`'s own note
        // (`cyrup-config/src/settings/store.rs:80-82`) about scrutinee borrows across an await.
        let written = settings
            .persist_nested(settings_scope, &[toggle.kind.key()], value)
            .await;
        if let Err(e) = written {
            persist_err = Some(e.to_string());
        }
```

Three separate problems, all of them the exact class this queue exists to remove:

1. **`FileSettingsStore::with_lock` has no such note.** It was deleted by the completed half of this
   task. The comment asserts the existence of a thing that is not in the tree.
2. **`cyrup-config/src/settings/store.rs:80-82` is a dangling citation.** Those three lines are now
   `}`, `}`, and a blank line at the tail of `FileSettingsStore::read`.
3. **"scrutinee borrows across an await" is the invented rule itself.** It is false here for the
   same reason it was false in `store.rs`: `persist_nested` returns an owned
   `Result<(), ConfigError>`, `persist_err` is a different capture from `settings`, and NLL ends the
   scrutinee borrow at the `.await`. Nothing forces the binding.

### Required change

Delete **only** those two `//` lines. The three-line `// Awaited HERE, before the loop redraws: …`
comment immediately above them is **true and must stay** — it explains a real ordering property
(each toggle is on disk before the selector repaints), not a borrow-check rule.

Anchor (`src.count(old) == 1`, verified 2026-08-23):

```rust
        // Bound before the `if let`, matching `FileSettingsStore::with_lock`'s own note
        // (`cyrup-config/src/settings/store.rs:80-82`) about scrutinee borrows across an await.
        let written = settings
```

Replace with:

```rust
        let written = settings
```

Inlining the `let written` binding into the `if let` is **optional**, not required. If you do it,
the result must still pass `cargo check -p cyrup` and
`rustfmt --check --edition 2024 crates/cyrup/src/subcommands.rs` (the inline form is over 100
columns on one line and would need wrapping). If in any doubt, leave the binding and just remove the
two comment lines — the falsehood is the defect, the binding is inert.

Verification:

- `grep -rn 'store.rs:80-82' --include=*.rs crates/ | grep -v '/target/'` → **no output**.
- `grep -rn 'scrutinee borrows across an await' --include=*.rs crates/ | grep -v '/target/'` → **no output**.
- `grep -c 'Awaited HERE, before the loop redraws' crates/cyrup/src/subcommands.rs` → **1** (still there).
- `cargo check -p cyrup` succeeds with no new warnings.
- `rustfmt --check --edition 2024 crates/cyrup/src/subcommands.rs` prints nothing.

## Outstanding item 2 — stop the queue from re-planting it

[`.flux/todo/MEDIUM-deferred-config-write-loses-toggles-on-err-and-replays-snapshots.md`](./MEDIUM-deferred-config-write-loses-toggles-on-err-and-replays-snapshots.md)
carries the same two lines **inside its prescribed "Replace with" block** (around lines 703-704 of
that task file). If that task is executed as written after this one, the falsehood comes straight
back.

Delete those two lines from that task file's "Replace with" code block. Change nothing else in it.

Note while you are in there (do not act on it, it is that task's business, not this one's): its
"Find (1 match — lines 873-905 …)" anchor describes the *pre-fix* shape — a `pending:
Vec<(SettingsScope, &'static str, serde_json::Value)>` plus a flush loop and a sync `|payload|`
closure. `subcommands.rs` on disk already has the post-fix shape (`async |payload: &str|`, the
awaited write in-loop), so that anchor matches **zero** times today. Leave that discrepancy for
whoever picks that task up.

## Do not touch

- `crates/cyrup-config/src/settings/store.rs` — finished and verified. No further edit of any kind.
- The three-line `// Awaited HERE, before the loop redraws: …` comment in `subcommands.rs`.
- The two true `FileSettingsStore::with_lock` prose mentions in
  `crates/cyrup-ext-subagents/src/discovery/settings_write.rs` (lines 74 and 261) — both say only
  that it uses `FileLock` + `write_atomic`, which is correct.
- The `with_lock` mention in `crates/cyrup-config/src/settings/tests/write_refusal.rs:145`.
- Tests. Nothing here changes behaviour; do not add or run any.
- No workspace-wide `cargo fmt`.

---

## Settled — verified by QA on 2026-08-23, do not re-verify or redo

All three prescribed replacements in `crates/cyrup-config/src/settings/store.rs` were applied
byte-exactly and are correct:

- `grep -n 'let next'` → no output. `grep -c '^\s*//[^/!]'` → **0**.
- `grep -c 'if let Some(new_text) = f(current.as_deref()) {'` → **1**;
  `grep -c 'if let Some(new) = f(guard.as_deref()) {'` → **1**.
- `grep -c "dyn for<'s> FnMut"` → **3**; bare `grep -c "for<'s>"` → **4**. Signatures unchanged.
- `grep -c 'is spelled out because'` → **1**. The rule is stated in exactly one place.
- `cargo check -p cyrup-config --all-targets` → clean, zero warnings. This is direct empirical proof
  of the top-left cell of the original §1 matrix: HRTB present, inline `if let`, 0 errors. The
  `let next` binding was indeed inert.
- `rustfmt --check --edition 2024 crates/cyrup-config/src/settings/store.rs` → silent.
- The four `with_lock` call sites in `manager.rs` are unchanged (now at `:233`, `:295`, `:343`,
  `:441` — the task body's `232/292/338/426` are stale because other passes moved `manager.rs`; the
  calls themselves were not touched). `store: Arc<dyn SettingsStore>` is still at `manager.rs:26`, so
  the trait-level rustdoc pointer remains correct.

The new eight-line rustdoc on `SettingsStore::with_lock` was checked **clause by clause against the
vendored `async-trait 0.1.89` source**, not taken on the task file's word. Every claim holds:

- "renames every elided `&` in the signature into a method-level named lifetime" — `lifetime.rs`
  `visit_opt_lifetime` (`:22-27`) mints via `next_lifetime` (`:37-42`, `'life{N}`), and `expand.rs`
  `for elided in lifetimes.elided` (`:238-243`) pushes each onto `sig.generics.params` with a
  `'lifeN: 'async_trait` predicate — i.e. an early-bound method generic the caller picks.
- "`CollectLifetimes::visit_type_reference_mut` recurses into the `Fn(..)` sugar as well" —
  `lifetime.rs:54-57` calls `visit_opt_lifetime` then `visit_mut::visit_type_reference_mut`, whose
  default walk reaches `TypeParen` → `TypeTraitObject` → `TraitBound` →
  `PathArguments::Parenthesized` → `Option<&str>` → the inner `&str`. The explicit `'s` instead
  lands in `explicit` via `visit_lifetime` (`:29-35`) and is filtered out by `Context::lifetimes`
  (`expand.rs:40-53`) against the trait's own (empty) generics, so it is left untouched.
- "`+ Send` is load-bearing … `&mut T: Send` only when `T: Send`" — correct; async-trait's non-local
  expansion boxes as `Pin<Box<dyn Future + Send + 'async_trait>>`.

Two harmless drifts, explicitly **not** defects — do not "fix" them:

- DoD item 7 expects `wc -l` = **141**; the file is **153**. The extra 12 lines are the
  `**[CYRUP-DELTA]** The trait is HALF async on purpose…` rustdoc paragraph (`store.rs:13-24`) added
  to the trait by a different task after this task's research was written. The +2 delta this task was
  responsible for is present and correct.
- The `manager.rs` line numbers in DoD item 10, as above.
