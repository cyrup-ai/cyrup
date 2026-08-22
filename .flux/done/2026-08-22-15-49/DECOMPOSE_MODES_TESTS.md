---
stage: qa
status: completed
updated: 2026-08-22 18:15
---

# modes.rs comment rework — one line left

## QA verdict: 9/10

Four of the five edits are exactly right and are **done — do not touch them again**:

- `rpc_ui_effects.rs:16-18` — the doc-comment "above" now reads "the blocking
  `confirm`/`input`/`select`/`editor` **half of the capability**, whose transport
  [`super::rpc_ui_dialogs`] exercises". This is the careful phrasing: it names pi's capability half
  and says the module exercises its transport, without claiming each of the four has a case.
- `rpc_ui_effects.rs:1` — module doc names the sibling half.
- `rpc_ui_dialogs.rs:204` — "in this file" → "in the whole modes suite".
- `rpc_commands.rs:3-5` — `compact`'s refusal path folded into the enumeration (better than the
  task's suggested append, which would have made a 104-col line).

Independently verified, not taken on report: exactly the five known-good positional references
survive (`rpc_bash.rs:31,104,283`, `rpc_ui_dialogs.rs:202`, `rpc_ui_effects.rs:160`), all resolving
inside their own test body; 36 tests still present, per-module counts unchanged (5/3/5/6/2/5/4/3/1/2)
with assertion and `use` counts intact, so the diff really was comment-only; `cargo check -p
cyrup-modes --tests` warning-free; `cargo test --lib -- --list` still 36 names under `tests::modes::`;
every newly authored line ≤100 cols.

## Outstanding: `rpc_ui_effects.rs:40-41` claims a case that does not exist

```rust
    // `spawn_blocking` needed, unlike the blocking `confirm`/`input`/`select`/`editor` dialogs in
    // `rpc_ui_dialogs`).
```

"…`editor` dialogs **in** `rpc_ui_dialogs`" locates all four in that module. Only three are there —
`hs.select` (:44, :246), `hs.confirm` (:67, :187), `hs.input` (:87). **No `editor` case exists
anywhere under `modes/`** (`grep -rn '\.editor(' crates/cyrup-modes/src/tests/modes/` → nothing);
`HostServices::editor` lives at `crates/cyrup-ext/src/host/services.rs:221` and nothing here drives
it. A reader sent to that module looking for the `editor` dialog finds no such thing — which is the
same class of broken locator this whole rework exists to remove.

It is also self-inconsistent: the doc comment 24 lines above, in this very file, states the same
comparison correctly. The two must agree.

This came in from the task's own prescribed AFTER text, whose research section had already ruled the
shape out ("must NOT say 'the four cases in [`super::rpc_ui_dialogs`]' — that would replace a
dangling reference with a false one"). The spec contradicted itself; the correct move was to follow
the rule, not the sample.

### Fix

Match the phrasing that is already correct at `:16-18` — name the capability half, not a location for
four cases:

```rust
    // `spawn_blocking` needed, unlike the blocking `confirm`/`input`/`select`/`editor` half of the
    // capability, whose transport `rpc_ui_dialogs` exercises).
```

Plain backticks, no intra-doc link brackets — this is a `//` comment, which is never rendered, and
that matches how the surrounding body comments already refer to code.

## Definition of done

- `rpc_ui_effects.rs:40-41` no longer asserts that an `editor` dialog case lives in `rpc_ui_dialogs`,
  and reads consistently with the doc comment at `:16-18`.
- Both replacement lines stay ≤100 cols.
- Nothing else changes: no test body, assertion, `use`, `pi` citation, or other comment.
- Still clean afterwards:
  ```bash
  cargo check -p cyrup-modes --tests
  cargo test -p cyrup-modes --lib -- --list | grep -c '^tests::modes::'   # 36
  grep -rniE '\b(above|below|earlier|later|preceding|this file|sibling)\b' \
    crates/cyrup-modes/src/tests/modes/                                    # the same 5 hits
  ```
