---
stage: new
status: done
updated: 2026-08-22 16:59
---

# Decompose cyrup-permission-system src/extension.rs Into Submodules

## Description

`cyrup-permission-system` — `src/extension.rs` — 4,681 lines.

Decompose into submodules based on logical separation of concerns.

## Acceptance Criteria

- [ ] `src/extension.rs` is split into submodules under a `src/extension/` directory (or equivalent), each grouped by a single logical concern
- [ ] No behavior changes: public API surface of the crate is unchanged
- [ ] Crate builds cleanly and existing tests pass
