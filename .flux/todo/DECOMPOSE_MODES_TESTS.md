---
stage: exec
status: done
updated: 2026-08-22 17:35
---

# Decompose modes.rs Test File Into Submodules — comment rework

## State: the split is done. Only comment repair remains.

`crates/cyrup-modes/src/tests/modes.rs` (2,005 lines, 36 tests) is already decomposed into
[`modes/`](../../crates/cyrup-modes/src/tests/modes) — `mod.rs` + `support.rs` + 10 concern modules,
largest 339 lines. QA verified test-name parity (36/36 via `cargo test --lib -- --list` diffed with
the module prefix stripped), a warning-free `cargo check`, zero clippy findings in the new files, and
that the two dedups (`spawn_rpc_duplex`, `build_runtime` in the model tests) are behaviour-preserving.

**Do not re-split anything.** What is left is the one defect class a move like this creates and that
neither the compiler nor the test run can catch: a comment moved verbatim whose **positional**
reference no longer resolves. Verbatim is right for a `pi` citation; it is wrong for "above".

## Research

### The defect set is closed — five edits, no sixth

Scanned programmatically rather than by eye, two ways:

1. Every `fn`/`struct` name ≥12 chars defined anywhere under `modes/`, matched against every comment
   line in every file that does **not** define it → **0 hits**. No comment names a test or helper
   that now lives in a sibling module.
2. A positional-phrase sweep (`further down/up`, `the next test`, `the previous`, `elsewhere in`,
   `as above`, `see above`, `the first of`, `just below`, `earlier in`, `later in`, …) → one hit,
   `rpc_ui_dialogs.rs:124` "RED before the fix on the first of the three cases", which refers to
   three cases inside its own test body. Valid; leave it.

So the QA list is exhaustive. The five surviving `above`/`below`/`sibling` references that ARE
correct — `rpc_bash.rs:31`, `rpc_bash.rs:104`, `rpc_bash.rs:283`, `rpc_ui_dialogs.rs:202`,
`rpc_ui_effects.rs:157` — all resolve inside their own file or their own test body. **Do not touch
them.**

### The fix syntax already has house precedent in this repo

Do not write prose like "see the dialogs module". This workspace already links sibling test modules
with intra-doc links, in both `//!` and `///` position:

- [`crates/cyrup-it/tests/intercom/compose_send_leg.rs:5`](../../crates/cyrup-it/tests/intercom/compose_send_leg.rs) — ``//! reason as [`super::tool_actions`]: both spawn …``
- [`crates/cyrup-it/tests/intercom/compose_send_leg.rs:47`](../../crates/cyrup-it/tests/intercom/compose_send_leg.rs) — ``/// not [`super::common::registration`], which takes a name …``
- [`crates/cyrup-it/tests/permission/forwarding_common.rs:2`](../../crates/cyrup-it/tests/permission/forwarding_common.rs) — ``//! ([`super::forwarding_subprocess`] and [`super::forwarding_spawn_env`]) genuinely shared …``
- [`crates/cyrup-it/tests/support/scratch.rs:83`](../../crates/cyrup-it/tests/support/scratch.rs) — ``/// [`super::env::hermetic`] with this tree …``

Use ``[`super::rpc_ui_dialogs`]`` / ``[`super::rpc_ui_effects`]``.

No lint risk: the workspace `[lints]` table carries only `clippy::{unwrap_used, expect_used, panic,
indexing_slicing}` — there is no `[workspace.lints.rust]` or `[workspace.lints.rustdoc]` — and these
modules are `#[cfg(test)]`, so rustdoc never resolves the links at all.

### The trap: `editor` has no case, so do not claim it does

The comment being repaired names ``confirm``/``input``/``select``/``editor``. Only **three** of those
are exercised in [`rpc_ui_dialogs.rs`](../../crates/cyrup-modes/src/tests/modes/rpc_ui_dialogs.rs) —
`hs.select` (:44, :246), `hs.confirm` (:67, :187), `hs.input` (:87). There is **no** `editor` case;
`HostServices::editor` exists at
[`crates/cyrup-ext/src/host/services.rs:221`](../../crates/cyrup-ext/src/host/services.rs) but nothing
under `modes/` drives it.

The original four-name list is naming **pi's blocking dialog half of the `ui` capability**, not four
tests. So the rewrite must point at that half (whose transport is exercised in `rpc_ui_dialogs`) and
must NOT say "the four cases in [`super::rpc_ui_dialogs`]" — that would replace a dangling reference
with a false one.

## The five edits

All comment-only. Match the `BEFORE` text exactly; change nothing else on the line.

### 1. `modes/rpc_ui_effects.rs:15` — dangling "above" in the fire-and-forget doc

BEFORE (lines 15-16):

```rust
/// expected (`rpc-mode.ts:149-241`) — unlike `confirm`/`input`/`select`/`editor` above, none of these
/// calls block on a reply, so no `extension_ui_response` is ever sent back for them in this test.
```

AFTER:

```rust
/// expected (`rpc-mode.ts:149-241`) — unlike the blocking `confirm`/`input`/`select`/`editor` half of
/// the capability, whose transport [`super::rpc_ui_dialogs`] exercises, none of these calls block on
/// a reply, so no `extension_ui_response` is ever sent back for them in this test.
```

### 2. `modes/rpc_ui_effects.rs:38` — the same dangling "above", in a body comment

BEFORE (lines 36-38):

```rust
    // notify → `{method:"notify", message, notifyType}` (rpc-mode.ts:149-157). None of these calls
    // block: `HostServices::notify` is a plain sync fire-and-forget send, called directly (no
    // `spawn_blocking` needed, unlike `confirm`/`input`/`select`/`editor` above).
```

AFTER:

```rust
    // notify → `{method:"notify", message, notifyType}` (rpc-mode.ts:149-157). None of these calls
    // block: `HostServices::notify` is a plain sync fire-and-forget send, called directly (no
    // `spawn_blocking` needed, unlike the blocking `confirm`/`input`/`select`/`editor` dialogs in
    // `rpc_ui_dialogs`).
```

Body comments in this suite use plain backticks, not intra-doc links (`//` comments are not rendered),
which is why this one names the module without the `[` … `]` brackets.

### 3. `modes/rpc_ui_dialogs.rs:204` — "in this file" silently re-scoped

BEFORE (lines 201-204):

```rust
    // SEAM-030 — the `started.elapsed() < 2s` margin that used to follow is DELETED: it carried no
    // semantic content the `timeout(5s)` + `assert!(!resolved)` above does not already carry (the
    // dialog demonstrably settled on its own, unanswered), and it was the most flake-prone
    // assertion in this file.
```

AFTER — only the last line changes:

```rust
    // assertion in the whole modes suite.
```

"This file" meant all 2,005 lines when SEAM-030 was written; unqualified it now reads as a claim about
one 283-line module, which understates it.

### 4. `modes/rpc_ui_effects.rs:1` — name the other half

BEFORE (line 1):

```rust
//! The fire-and-forget half of the `ui` capability: `notify`/`setStatus`/`setWidget`/`setTitle`/
```

AFTER:

```rust
//! The fire-and-forget half of the `ui` capability (the blocking half is [`super::rpc_ui_dialogs`]):
//! `notify`/`setStatus`/`setWidget`/`setTitle`/
```

"Half" was self-evident when both halves shared one file. Reflow lines 1-2 so the prose still reads
as one sentence and no line exceeds the ~100-col width the rest of the file keeps.

### 5. `modes/rpc_commands.rs:1-4` — module doc under-describes the module

The header lists the prompt/abort/state core, `fork`, and the extended surface, but the module also
holds `rpc_compact_refusal_is_an_error_response_with_pi_s_reason`. Add it, e.g. append to the last
line: `… pin key-for-key, and `compact`'s refusal path.`

## Out of scope

- Any change to a test body, an assertion, an assertion message, a `use` statement, or a `pi` source
  citation. This is a comment-only diff.
- Re-splitting, re-grouping, or moving any test between modules.
- Reciprocal cross-links beyond edit #4 — `rpc_ui_dialogs`'s own doc stands on its own.
- The `rpc_cycle_model_spans_the_full_auth_filtered_registry` failure: it is the documented ambient
  AWS-credentials issue owned by [`TEST_FAILURES.md`](TEST_FAILURES.md) item 2 (it passes with
  `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` unset). Leave it.
- The 6 pre-existing clippy findings in `crates/cyrup-modes/src/rpc_client.rs`.

## Definition of done

- All five edits applied, `BEFORE` text gone, nothing else in the diff.
- Every surviving positional reference resolves inside its own file. Confirm with:
  ```bash
  grep -rniE '\b(above|below|earlier|later|preceding|this file|sibling)\b' \
    crates/cyrup-modes/src/tests/modes/
  ```
  Expect exactly the five known-good hits (`rpc_bash.rs:31,104,283`, `rpc_ui_dialogs.rs:202`,
  `rpc_ui_effects.rs:157`) plus whatever the new wording introduces, each resolving locally.
- `cargo check -p cyrup-modes --tests` stays warning-free.
- `cargo test -p cyrup-modes --lib -- --list` still reports the same 36 names under `tests::modes::`.
