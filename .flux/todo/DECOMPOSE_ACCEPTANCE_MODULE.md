---
stage: new
status: done
updated: 2026-08-22 19:30
---

# Decompose exec/acceptance.rs Into Submodules

## Description

decompose exec/acceptance.rs into submodules based on logical separation of concerns

`crates/cyrup-ext-subagents/src/exec/acceptance.rs` is now the largest `.rs` file in the
crate at 9,599 lines, following the `extension.rs` decomposition. Break it into an
`acceptance/` module tree organized by logical separation of concerns.

## Shape of the file (measured, not assumed)

The line count is misleading and the plan must not be scoped from it:

| | lines |
|---|---:|
| total | 9,599 |
| **production** | **1,827** |
| inline `#[cfg(test)] mod tests` | 7,773 (81%) |

40 top-level items across 1,827 production lines. This is a normal-sized module carrying a
very large test suite — the opposite balance from `extension.rs` (13,470 production /
8,945 test). Any plan that treats this as "split 9,600 lines" is mis-scoped.

**This means the plan must decide, explicitly and with reasoning, what the deliverable is:**

- whether the production code genuinely has six separable concerns, or whether the honest
  finding is "one coherent module with an oversized test file";
- if the latter, whether the right change is a test-only relocation (a `#[path]` sibling, or
  a `tests` submodule) rather than a production split;
- if a production split is warranted, that ~1,827 lines across 6 modules is ~300 lines each,
  which may be too fine a granularity to be worth the import and visibility cost.

Do not assume the answer is a six-way split. Reaching "the production code should stay as one
module, and only the tests move" is an acceptable and possibly correct outcome.

## The author's own seams

As with `extension.rs`, the file carries banner comments marking its sections. These are the
strongest available evidence for where concern boundaries lie:

| Line | Section |
|---|---|
| 77 | The ordered provenance lattice (func-SA §4.3, arch-SA §3.4) |
| 271 | R-SA-023: Acceptance contract injection |
| 682 | Lowering a raw wire `acceptance` value onto an `AcceptanceContract` (SUBA-041 / SUBA-N04) |
| 1099 | R-SA-032 / DI-SA-5: `verify[]` REAL subprocess execution |
| 1287 | R-SA-032: acceptance-gate evaluation |
| 1622 | G82: the child-authored output file as an acceptance-report source |

## Scope of the plan

- Inventory every top-level item and map it to a concern (or record that it belongs to the
  module's single concern).
- A recommendation on production-split vs test-relocation, with the reasoning stated.
- If splitting: concrete target paths, item assignments, visibility decisions, and an
  ordering where each step compiles independently.
- Test placement: where the 7,773 lines go, and whether any become integration tests.
- Public API preservation — `acceptance` is `pub` within `exec`, and `crate::extension`
  imports `lower_acceptance_input` from it, so the re-export surface must be enumerated and
  held.
- Risks: circular dependencies, shared state, and any item whose concern is genuinely
  ambiguous.

## Prior art

The `extension.rs` decomposition (PR #35, merged) established the working method for this
crate: split at the author's banner comments; define each type in its subtree's `mod.rs` so
`impl` blocks in child modules keep private-field access; widen cross-module items to
`pub(crate)` only; co-locate tests with the code they exercise; prove content preservation
with a line-level comparison rather than asserting it. Its completed task file is at
`.flux/done/2026-08-22-13-09/DECOMPOSE_EXTENSION_MODULE.md`.

## Acceptance Criteria

- [ ] The plan states, with reasoning, whether the production code warrants splitting at all
      or whether the finding is "one module, oversized test suite".
- [ ] Every top-level item is accounted for — mapped to a target module, or explicitly
      recorded as staying put.
- [ ] Public API of `crate::exec::acceptance` is unchanged; the re-export surface and its
      cross-module consumers (at minimum `crate::extension`'s `lower_acceptance_input`) are
      enumerated.
- [ ] Test placement is decided for all 7,773 lines.
- [ ] If a split is proposed, it is sequenced into independently compilable steps and the
      risks are named with mitigations.
