---
stage: new
status: done
updated: 2026-08-22 15:38
---

# Decompose ctx.rs Into Submodules By Separation Of Concerns

## Description

decompose crates/cyrup-ext-sdk/src/ctx.rs in submodules based on logical separation of concerns

`crates/cyrup-ext-sdk/src/ctx.rs` is the largest Rust file in the crate (1,633 lines / 73 KB).
Break it into a `ctx/` submodule directory whose files each own one logical concern, keeping
the crate's public API unchanged.

## Acceptance Criteria

- [ ] `crates/cyrup-ext-sdk/src/ctx.rs` is replaced by a `crates/cyrup-ext-sdk/src/ctx/` directory
      with `mod.rs` (or an equivalent `ctx.rs` + `ctx/` layout) re-exporting the same items
- [ ] Submodule boundaries follow logical separation of concerns, not arbitrary line splits
- [ ] The crate's public API surface is unchanged — every path previously reachable as
      `cyrup_ext_sdk::ctx::*` still resolves
- [ ] No behavior changes: this is a pure move/split refactor
- [ ] `cargo build` and `cargo clippy` pass with no new warnings
- [ ] Existing tests (including `src/tests/`) pass unchanged
