---
stage: new
status: done
updated: 2026-08-22 15:49
---

# Decompose modes.rs Test File Into Submodules

## Description

decompose crates/cyrup-modes/src/tests/modes.rs into submodules based on logical separation of concerns

`crates/cyrup-modes/src/tests/modes.rs` is the largest Rust source file in the
`cyrup-modes` crate at ~2,005 lines / 95 KB. Split it into focused submodules
under `crates/cyrup-modes/src/tests/modes/`, grouping tests by the concern they
exercise rather than by arbitrary line count, and leave `modes.rs` (or a new
`modes/mod.rs`) as the module declaration + any shared test helpers.

## Acceptance Criteria

- [ ] Test groupings are derived from the actual concerns exercised in the file, not arbitrary splits
- [ ] Shared test helpers/fixtures live in one place and are reused, not duplicated per submodule
- [ ] No test is dropped, renamed away, or silently disabled — the same set of tests exists before and after
- [ ] `cargo test -p cyrup-modes` passes with the same results as before the split
- [ ] No submodule remains disproportionately large; each covers one clear concern
- [ ] Change is test-only — no production source under `crates/cyrup-modes/src/` outside `tests/` is modified
