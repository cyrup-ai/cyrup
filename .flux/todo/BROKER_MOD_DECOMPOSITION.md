---
stage: new
status: done
updated: 2026-08-22 15:43
---

# Decompose Broker Mod Into Submodules

## Description

decompose src/broker/mod.rs into submodules based on logical separation of concerns

`crates/cyrup-intercom/src/broker/mod.rs` is the single largest Rust source file in the
`cyrup-intercom` crate at 3,292 lines / 156 KB — roughly 15% of the crate's 21,364 total lines,
and nearly 2x the next largest file (`src/transport/client.rs`, 1,705 lines).

The `broker/` directory already has sibling submodules (e.g. `runtime_claim.rs`, `listener.rs`),
so the pattern for splitting exists; `mod.rs` itself has simply accumulated too much. Break it
apart along its natural seams into cohesive submodules, leaving `mod.rs` as a thin module
declaration + re-export surface plus whatever small amount of genuinely cross-cutting glue
cannot sensibly live elsewhere.

This is a pure refactor: no behavior changes, no API changes visible outside the crate.

## Acceptance Criteria

- [ ] `src/broker/mod.rs` is reduced to module declarations, re-exports, and minimal glue
- [ ] Each new submodule groups a single, nameable concern; module boundaries follow how the
      code actually clusters, not an arbitrary line-count target
- [ ] The crate's public API is unchanged — existing paths still resolve via re-exports, and
      no downstream crate in the workspace needs edits
- [ ] `cargo build` and `cargo clippy` are clean (no new warnings)
- [ ] Existing tests pass unchanged; tests move with the code they cover
- [ ] No logic is rewritten during the move — code is relocated, with only visibility and
      `use` statements adjusted as needed
