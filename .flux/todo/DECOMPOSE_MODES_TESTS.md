---
stage: qa
status: needs-rework
updated: 2026-08-22 16:55
---

# Decompose modes.rs Test File Into Submodules — QA rework

## QA verdict: 9/10

The split itself is done and correct. **Do not redo it.** Verified in review:

- `crates/cyrup-modes/src/tests/modes.rs` is gone; `modes/mod.rs` + 11 submodules exist, largest
  339 lines, `mod.rs` declarations only.
- All 36 tests present, one per planned module (5/3/5/6/2/5/4/3/1/2), no rename, no `#[ignore]`.
- Line accounting reconciles to the original 2,005 exactly: every line is either in a submodule, in
  `mod.rs` (suite doc + the `#![allow]`), redistributed into a per-module `use` block, an inter-item
  blank, or one of the four label-only section banners (`Fixture` / `PRINT mode` / `JSON mode` /
  `RPC mode`) whose entire content is now carried by the module name and its `//!` doc. No
  citation-bearing or rationale-bearing comment was dropped.
- The single `#![allow(clippy::unwrap_used, expect_used, panic, indexing_slicing)]` sits in `mod.rs`
  and is the only `allow` in the subtree.
- `pub(super)` visibility on the moved fixtures is correct and minimal; `Fixture::_tmp` stayed
  private.
- Both dedups are sound: `spawn_rpc_duplex` reproduces the six inlined blocks exactly (binding drop
  order unchanged), and the two model tests' inlined construction really was line-for-line
  `build_runtime`'s body — the duplicate `AnyFauxResolver` that rode along with the SEAM-004 banner
  is correctly gone from `rpc_models.rs` and lives once in `support`.
- `cargo check -p cyrup-modes --tests` clean, zero warnings. Clippy reports nothing in
  `tests/modes/` (the 6 findings in `src/rpc_client.rs` and those in dependency crates are
  pre-existing and out of scope).
- Test-name parity proven by diffing `cargo test --lib -- --list` before/after with the module
  prefix stripped: identical, 36/36; crate total unchanged at 75.
- The one red case, `rpc_cycle_model_spans_the_full_auth_filtered_registry`, is the documented
  ambient-AWS-credentials failure already queued as `TEST_FAILURES.md` item 2 — proven unrelated by
  ablation (passes with `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` unset; suite is then 75/75).
  Correctly left alone.

## Outstanding — three comment defects the move introduced

Moving a comment verbatim is right for a citation and wrong for a **positional** reference: two
comments now point "above" at tests that live in a different module. This is the one class of
breakage a split like this creates, and it is not caught by the compiler or the test run.

### 1. `modes/rpc_ui_effects.rs:15` — dangling "above"

The doc on `rpc_fire_and_forget_ui_effects_reach_the_wire` reads:

```rust
/// expected (`rpc-mode.ts:149-241`) — unlike `confirm`/`input`/`select`/`editor` above, none of these
```

Those four dialog cases are no longer above — they are in `modes/rpc_ui_dialogs.rs`. Point the
reference at the module that now holds them, e.g.:

```rust
/// expected (`rpc-mode.ts:149-241`) — unlike the blocking `confirm`/`input`/`select`/`editor` calls
/// in [`super::rpc_ui_dialogs`], none of these calls block on a reply, so no `extension_ui_response`
/// is ever sent back for them in this test.
```

Keep every citation and every claim; only the locator changes.

### 2. `modes/rpc_ui_effects.rs:38` — the same dangling "above"

```rust
    // `spawn_blocking` needed, unlike `confirm`/`input`/`select`/`editor` above).
```

Same fix: name `rpc_ui_dialogs` instead of "above".

### 3. `modes/rpc_ui_dialogs.rs:204` — "in this file" silently re-scoped

```rust
    // dialog demonstrably settled on its own, unanswered), and it was the most flake-prone
    // assertion in this file.
```

"This file" meant the whole 2,005-line suite when the SEAM-030 note was written; it now reads as a
claim about `rpc_ui_dialogs.rs` alone. Say what was meant — "the most flake-prone assertion in the
modes suite" — so the historical claim survives the split intact.

### 4. `modes/rpc_commands.rs:1-4` — module doc under-describes the module

The header enumerates the prompt/abort/state core, `fork`, and the extended command surface, but the
module also holds `rpc_compact_refusal_is_an_error_response_with_pi_s_reason`. Add the compact
refusal to the list so the doc matches the file's contents.

## Definition of done for this rework

- No comment anywhere under `crates/cyrup-modes/src/tests/modes/` refers to a test by position when
  that test is in a different module. Re-check with:
  `grep -rniE '\b(above|below|earlier|later|preceding|this file|sibling)\b' crates/cyrup-modes/src/tests/modes/`
  — every surviving hit must resolve **inside its own file** (the four in `rpc_bash.rs:31,104,283`
  and `rpc_ui_dialogs.rs:202`, `rpc_ui_effects.rs:157` already do; leave them alone).
- `cargo test -p cyrup-modes --lib -- --list` still reports the same 36 names.
- `cargo check -p cyrup-modes --tests` stays warning-free.
- Comment-only change: no test body, assertion, or `use` statement is touched.
