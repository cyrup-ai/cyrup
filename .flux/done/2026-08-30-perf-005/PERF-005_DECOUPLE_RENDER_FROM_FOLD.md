---
stage: qa
status: completed
updated: 2026-08-30 01:52
---

# PERF-005 — outstanding after QA round 3

**Rated 9/10.** All three items landed, both spec deviations were correctly identified and are
correct. Gate green: clippy **exit 0**, `cargo doc --workspace --no-deps` **exit 0**, tests
**8,331 passed / 0 failed / 11 ignored**.

Three items remain, all inside
[`highlight.rs`](../../../crates/cyrup-tui/src/markdown/highlight.rs). Items 1 and 2 are the same
one-line fix. Item 3 is a stale comment.

Everything under **"Verified good — do not redo"** is settled. Do not revisit it.

## 1. Rule (a) is documented but not enforced: a tool body can still claim the cursor

The `memoable` arm's comment states the guarantee:

> A settled tool body matches no cursor's language or consumed prefix, so it still takes the uncached
> path — several of them render per frame and none may reset the cursor the growing fence depends on.

That is false in a window that occurs on **every streamed fence**. The inherit test is

```rust
                && stable.len() >= c.consumed_text.len()
                && stable.starts_with(c.consumed_text.as_str())
```

and `stable.starts_with("")` is true for every input. A fresh cursor is created with
`consumed_text: String::new()`, and `advance_cursor` only advances it past a `\n` — so while a
streaming fence is still on its **first line** (`code` has no newline, `stable` is `""`), its cursor's
consumed prefix is empty and *any* block sharing its language matches.

`highlight_code_lines` is `memoable`, so a `read`/`write` body of a `.rs` file
([`tool_builtin.rs:64`](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs),
[`:117`](../../../crates/cyrup-tui/src/transcript/tool_builtin.rs)) rendering in the same frame as a
`` ```rust `` fence that has just opened will inherit that cursor and then, in the success arm, set
`self.cursor = None` — destroying it. The fence rebuilds next frame.

The rows returned are correct in every case (syntect is forward-only, so the first `n` rows of a
prefix are the first `n` rows of the whole), and the blast radius today is one rebuilt one-line
cursor. But this is exactly the interference rule (a) exists to prevent, the code asserts it cannot
happen, and it is what makes item 2 reachable.

## 2. The `max_rows` break desynchronises `consumed_text` from `rows`

In `advance_cursor`:

```rust
            for raw in new_text.strip_suffix('\n').unwrap_or(new_text).split('\n') {
                if cursor.rows.len() >= max_rows {
                    break;
                }
                …push a row…
            }
            cursor.consumed_text.push_str(new_text);
```

The `push_str` is unconditional, so a bounded call that breaks early leaves the cursor claiming to
have consumed text whose rows it never parsed. Any later inherit of that cursor would splice a delta
onto the wrong state and return **wrong rows**.

This was unreachable before this round: only `highlight_lines` created or advanced a cursor, always
with `max_rows: usize::MAX`, so the break was dead code. Inheriting made the bounded path reachable
with a cursor for the first time (item 1 is how). It does not misbehave *today* only because the
`memoable` success arm drops the cursor immediately afterwards — an implicit, uncommented coupling
between two arms, and the kind that survives exactly until someone moves the drop.

### The fix for 1 and 2 is the same

Inheritance should require the cursor to have been built for the **same bound**, not merely the same
language and a prefix. `MemoKey` already keys on `max_rows` for this reason; the cursor should too.

Add the field to `HighlightCursor` beside `lang`/`theme_generation`, set it where the cursor is
created, and add one clause to the inherit test:

```rust
        let inherits = self.cursor.as_ref().is_some_and(|c| {
            c.lang == lang
                && c.theme_generation == theme.generation
                // The bound is part of the cursor's identity: a cursor advanced under a SMALLER
                // bound stops parsing early while still recording the text as consumed, so a later
                // inherit would splice onto state that was never built. It also restores rule (a) —
                // a fence carries `usize::MAX` and a tool body carries `shown`, so a settled body
                // can no longer match a growing fence through an empty consumed prefix.
                && c.max_rows == max_rows
                && stable.len() >= c.consumed_text.len()
                && stable.starts_with(c.consumed_text.as_str())
        });
```

A fence carries `usize::MAX` at both the open and closed call (`highlight_lines` passes it in both),
so the close-frame inherit this round added is unaffected. A tool body carries `shown` and can no
longer match. The `break` returns to being dead for every cursor that is ever inherited.

State the invariant where it can be checked — `advance_cursor`'s doc — rather than leaving it implied
by the caller: *the cursor is only ever advanced under the bound it was built for, so the early
`break` cannot desynchronise it.*

## 3. `advance_cursor`'s opening comment contradicts its own tail arm

```rust
        // Parse only what is new. On any syntect fault the cursor is poisoned — drop it and fall
        // back to the uncached path, which reproduces today's whole-block `None` exactly.
```

"On any syntect fault" is no longer true, and the method's own doc four lines above says so
correctly: a fault while consuming `stable` drops the cursor, a fault on `tail` does not, because
the tail is parsed on a clone. The tail arm two dozen lines below states the same thing again. Only
this comment still claims both faults poison the cursor.

Reword it to cover the `stable` side alone — the tail arm carries its own explanation and does not
need to be described from up here.

## Definition of done

1. `HighlightCursor` records the `max_rows` it was built under; `inherits` requires it to match.
2. A settled tool body cannot inherit a growing fence's cursor, including while that fence's
   consumed prefix is empty.
3. `advance_cursor`'s doc states that the cursor is only advanced under the bound it was built for,
   so the early `break` cannot leave `consumed_text` ahead of `rows`.
4. The opening comment describes only the `stable`-side fault.
5. Clippy, `cargo doc --workspace --no-deps` and the workspace test run stay clean. The
   three-phase equivalence guard must still pass — phase 2 is what pins the close-frame inherit,
   and the `max_rows` clause must not break it.

## Verified good — do not redo

- **The inherit-never-create restructure.** Correct, and the prefix-over-equality reasoning is right:
  one delta can carry both new body lines and the closing delimiter.
- **Dropping the slot on promotion**, and the two spec deviations, both correctly identified:
  DoD 3's literal wording would have cleared the cursor for settled tool bodies — rule (a)'s own
  failure mode — and the tail fault legitimately keeps the cursor because the tail parses on a clone.
- **The three-phase equivalence guard.** Catching that the old alternating loop stopped covering
  multi-line resume the moment promotion landed — green forever, testing nothing — was the sharpest
  find of the round, and proving phase 2 non-vacuous by corrupting the inherit branch and watching
  it fail at `the closing frame` is the right standard.
- **The `Rc` removal**, and the corrected `rows_for` doc: per-document scope, the three render sites
  sharing one thread-local, and the two reasons one slot suffices.
- **Everything from rounds 1 and 2** — `prepass::fence_is_closed` and its two guards, the walker
  plumbing, the `frame_due` arm position, the hardened priority guard, `token` at both
  `highlight_inner` callers, the `body_line` doc links, and `cargo doc` in the gate.
