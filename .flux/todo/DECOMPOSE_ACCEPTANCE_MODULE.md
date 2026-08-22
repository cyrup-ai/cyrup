---
stage: exec
status: done
updated: 2026-08-22 22:30
---

# Decompose exec/acceptance.rs Into Submodules

## Description

decompose exec/acceptance.rs into submodules based on logical separation of concerns

## ⚠️ The original task's premise was wrong — corrected here

The capture-stage task recorded **"1,827 production / 7,773 test (81%)"** and framed the central
question as *split vs. test-relocation*. That measurement was produced by splitting the file at the
first `#[cfg(test)]` and attributing everything after it to tests. It is wrong, and the error is
large: it swallowed a **4,166-line production module**.

Correct accounting, from locating every `#[cfg(test)] mod` at any nesting depth:

| | lines |
|---|---:|
| total | 9,599 |
| `#[cfg(test)] mod tests` @ 1828–3619 | 1,792 |
| `#[cfg(test)] mod tests` @ 7821–9598 (nested, inside `model`) | 1,778 |
| **production** | **6,030** |

So this is **not** "one module with an oversized test suite." It is 6,030 lines of production code
— the third-largest production module in the crate — and the test-relocation option the original
task floated does not apply.

## The decisive finding: two production acceptance implementations, mid-collapse

[`exec/acceptance.rs:3655`](../../crates/cyrup-ext-subagents/src/exec/acceptance.rs) opens
`pub mod model { … }`, an inline **production** module of 4,166 lines. Its own banner states the
situation in the author's words:

> The enum-lattice API above (`AcceptanceStatus`/`AcceptanceContract`/`evaluate_acceptance`) is the
> crate's original acceptance subsystem, wired into `exec/mod.rs::run_sync`. This module is the
> diagnosed-missing C12 port […]
>
> **UPSTREAM HAS ONE ACCEPTANCE IMPLEMENTATION. The two-API split is this crate's own accretion and
> is unfinished port work, not a design. It is being COLLAPSED onto this module, layer by layer.**

The banner records exactly how far the collapse has got — the verify runner, the verify result,
`validateAcceptanceInput` and `evidenceStatus` are already single-implementation — and what remains:

> What has NOT collapsed: the LEDGER struct and the verdict that fills it. […] Finishing the
> collapse means `exec/mod.rs::run_sync` calling `evaluate_acceptance` here and
> `SingleResult.acceptance` becoming `AcceptanceLedger` here.

`pub type VerifyCommand = model::AcceptanceVerifyCommand;` in the lattice half is the collapse
visible in the source: the old API already aliases into the new one.

**Both halves have live consumers.** Enum-lattice: `exec/mod.rs`, `background/runner_main.rs`,
`discovery/chains.rs`, `extension/executor/{background,requests}.rs`, `extension/tool/{routing,schema}.rs`,
plus `crates/cyrup-it/tests/`. Model: `exec/mod.rs`, `discovery/chains.rs`,
`background/spawn_detached.rs`, `spawn/chain_graph.rs`, `extension/tool/params.rs`, plus two of this
crate's own `src/tests/` files. Neither is dead.

**No queued task covers finishing the collapse** — grepping `.flux/todo/` for `C12`, `two-API` and
`collapse` returns nothing. That work is currently untracked.

## Decision: split, and shape the split to SERVE the collapse

Three options were weighed. This is the one to implement.

**Rejected — do nothing until the collapse lands.** The collapse is a large, behaviour-changing
project (it rewires `run_sync` and changes `SingleResult.acceptance`'s type). Blocking a mechanical,
behaviour-preserving cleanup behind it leaves the crate's third-largest production file untouched
indefinitely.

**Rejected — split by concern, interleaving both APIs.** Grouping "all the parsing", "all the verify
running" across both halves would fuse the two implementations into shared modules. That is actively
harmful: it destroys the boundary the collapse has to delete along, and makes "what is left to
remove" unanswerable.

**Required — split along the two-API boundary first, then by concern within each half.** The
accretion becomes two sibling directories instead of one scroll. Three things follow:

- what remains to collapse becomes a *diff between two directories*, not an archaeology exercise;
- when the collapse finishes, retiring the old API is **deleting `lattice/`**, not excising ranges
  from a 9,600-line file;
- the `lattice/` name states the temporary status in the tree itself, so nobody mistakes the
  accretion for a design.

## Target module tree

The author has already marked every seam: 6 banner comments in the lattice half, **18** inside
`model`. Use them. Line counts are production only; tests co-locate on top.

```
exec/acceptance/
├── mod.rs                    ~90   the existing 69-line module doc + the facade re-exports
│
├── lattice/                        the crate's ORIGINAL API — deleted when the collapse lands
│   ├── mod.rs                ~210  AcceptanceStatus (+ both impl blocks), AcceptanceLedger,
│   │                               default_pending_evidence_status, build_timed_out_acceptance_ledger
│   ├── contract.rs           ~410  AcceptanceContract + impl, VerifyCommand alias, ReviewerResult,
│   │                               clamp_requestable_level
│   ├── lowering.rs           ~330  lower_acceptance_input, lower_verify_command,
│   │                               LoweredAcceptancePolicy (+impl), lower_acceptance_policy,
│   │                               lower_criterion
│   ├── inject.rs             ~65   ACCEPTANCE_CONTRACT_HEADING, inject_acceptance_contract
│   ├── verify.rs             ~185  run_verify_commands{,_memoized,_memoized_with_cancel},
│   │                               spawn_pipe_drain, drained, drained_by, shell_command
│   ├── gate.rs               ~330  CleanCompletionGate (+impl), evaluate_acceptance{,_with_cancel},
│   │                               declared_structural_failures
│   ├── report_source.rs      ~115  AcceptanceFileOutput, select_acceptance_report_source,
│   │                               self_report_floor, ParsedAcceptanceReport, extract_acceptance_report
│   └── post_hoc.rs           ~85   PostHocCorrection, ACCEPTANCE_REJECTED_EXIT_CODE,
│                                   apply_post_hoc_correction
│
└── model/                          the C12 port — the SURVIVOR
    ├── mod.rs                ~40   module banner (keep the collapse-status text verbatim) + decls
    ├── types.rs              ~570  every enum and shape: AcceptanceLevel..AcceptanceLedger,
    │                               SerializableGate (banners @3664, 3776, 3907, 3934)
    ├── level.rs              ~375  required_evidence_for_level, infer_level,
    │                               normalize_acceptance_input, resolve_effective_acceptance
    │                               (banners @4238, 4465)
    ├── prompt.rs             ~145  ACCEPTANCE_REPORT_EXAMPLE, acceptance_requires_child_report,
    │                               format_acceptance_prompt (banner @4611)
    ├── report/
    │   ├── mod.rs            ~10   decls
    │   ├── fences.rs         ~150  FenceMatch, fenced_matches, fenced_block_bodies,
    │   │                           extract_balanced_json, parse_report_json (banner @4757)
    │   ├── normalize.rs      ~305  G79 report normalization (banner @4908)
    │   ├── validate.rs       ~325  validate_acceptance_report — exact error-message parity
    │   │                           (banner @5213)
    │   └── parse.rs          ~440  parse_acceptance_report*, strip_acceptance_report*
    │                               (banner @5538)
    ├── checks.rs             ~190  report_evidence_present, check_criteria_satisfied,
    │                               check_no_staged_files, run_structural_checks (banner @5977)
    ├── aggregate.rs          ~205  AggregateChild, aggregate_acceptance_report, ledger_status_str
    │                               (banner @6166)
    ├── verify/
    │   ├── mod.rs            ~10   decls
    │   ├── redact.rs         ~140  G80 secret redaction (banner @6370)
    │   ├── memo.rs           ~360  G80 per-workspace memoization (banner @6508)
    │   └── run.rs            ~300  run_verify_command{,_with_cancel} — REAL subprocess
    │                               (banner @6867)
    ├── evaluate.rs           ~295  EvaluateAcceptanceInput, evaluate_acceptance,
    │                               acceptance_failure_message (banner @7167)
    └── validate_input.rs     ~360  validate_acceptance_input + its three sub-validators
                                    (banner @7463)
```

**23 files, ~6,030 production lines, mean ~260.** Tests co-locate per the `extension.rs` precedent,
bringing the mean to ~415.

## API preservation contract

`exec/mod.rs:39` declares `pub mod acceptance;` and `lib.rs` declares `pub mod exec;`, so **both**
halves are public crate API. The surface to hold, verified by count:

- **18 `pub` items** at the lattice half's top level (`AcceptanceStatus`, `AcceptanceLedger`,
  `build_timed_out_acceptance_ledger`, `VerifyCommand`, `AcceptanceContract`, `ReviewerResult`,
  `lower_acceptance_input`, `inject_acceptance_contract`, `run_verify_commands`,
  `run_verify_commands_memoized`, `run_verify_commands_memoized_with_cancel`, `CleanCompletionGate`,
  `evaluate_acceptance`, `evaluate_acceptance_with_cancel`, `AcceptanceFileOutput`,
  `PostHocCorrection`, `ACCEPTANCE_REJECTED_EXIT_CODE`, `apply_post_hoc_correction`)
- **64 `pub` items** inside `model`
- the module path `crate::exec::acceptance::model` itself, which `spawn/chain_graph.rs`,
  `discovery/chains.rs`, `extension/tool/params.rs` and `src/tests/` name directly

`acceptance/mod.rs` re-exports the 18 at the same paths; `model` stays a `pub mod` so its 64 keep
their existing paths unchanged. **82 public items, zero consumer edits.**

## Method — reuse what is already proven in this crate

The `extension.rs` decomposition (PR #35, merged; task at
[`.flux/done/2026-08-22-13-09/DECOMPOSE_EXTENSION_MODULE.md`](../done/2026-08-22-13-09/DECOMPOSE_EXTENSION_MODULE.md))
established a method that worked on a harder case. Apply it unchanged:

1. **Split at the author's banners**, never at line counts.
2. **Define each type in its subtree's `mod.rs`**, put its `impl` blocks in that subtree's children.
   A private field is visible in its module *and all descendants*, so field access survives with no
   widening. Here that matters for `AcceptanceContract` (its `impl` at `:370` is 285 lines and
   belongs in `contract.rs` beside the struct) and for `AcceptanceVerifyResult` (struct at `:4021` in
   `types.rs`, `impl` at `:6909` in `verify/run.rs` — **this one crosses subtrees**, so either the
   `impl` moves to `types.rs` or the fields it touches become `pub(crate)`; prefer moving the `impl`).
3. **Widen only to `pub(crate)`**, and only where the compiler demands it. Drive it from
   `cargo check` output rather than pre-emptively.
4. **Co-locate each test with the code it exercises.** Where a test module would push a file past
   ~1,500 lines, use a `#[cfg(test)] #[path = "…_tests.rs"] mod tests;` sibling — the module path is
   unchanged.
5. **Prove content preservation mechanically**: a line-level multiset comparison of the original
   against the union of the new tree, with every residual difference explained. Assertion is not
   proof; the comparison is what caught real defects last time.
6. **Repair intra-doc links after the move.** The previous split broke 52 of them — links that
   resolved inside one module stop resolving across a tree. Run `cargo doc` before and after and
   compare counts.

## Sequencing

Each step compiles independently.

1. `mv exec/acceptance.rs exec/acceptance/mod.rs` — no content change.
2. Extract `model` **first**, as a whole: `pub mod model { … }` inline → `model/mod.rs`. This is the
   cheapest possible step (it is already a module; delete the wrapper and dedent by four) and it
   alone drops `mod.rs` from 9,599 to ~5,430.
3. Split `model/` by its 18 banners, leaves first — `types.rs` before anything referencing those
   types; `report/` and `verify/` subtrees before `evaluate.rs`, which uses both.
4. Split `lattice/`, leaves first: `post_hoc.rs`, `report_source.rs`, `inject.rs`, then
   `lowering.rs`, `verify.rs`, `contract.rs`, `gate.rs`, then `mod.rs`.
5. Write the facade in `acceptance/mod.rs`; verify the 82-item surface.
6. Relocate tests alongside their code.

## Risks

- **R1 — the `AcceptanceVerifyResult` impl crosses subtrees** (`types.rs` ↔ `verify/run.rs`). Named
  in the method above; move the `impl` to sit with the struct.
- **R2 — `model` names deliberately shadow lattice names** (`AcceptanceLedger`, `evaluate_acceptance`,
  `ParsedAcceptanceReport` exist in both, which the banner says is why they live under `model`). Do
  not glob-import across the two subtrees; use qualified paths, and never `pub use` a `model` name
  into `acceptance`'s root where it would collide with the lattice one.
- **R3 — the collapse is in flight.** Anyone finishing it will be editing these files. Land this
  before starting the collapse, or rebase the collapse onto it — not the reverse, since rebasing a
  behaviour change across a 23-file split is far harder than rebasing a split across a behaviour
  change.
- **R4 — both halves are public API** with consumers in four sibling modules and two other crates.
  The workspace-wide `cargo check --all-targets` is what proves the surface held; a crate-scoped
  check proves nothing.

## Definition of done

- `exec/acceptance.rs` no longer exists; `exec/acceptance/` holds the tree above, each file opening
  with a `//!` doc naming its concern.
- `acceptance/mod.rs` contains the module doc, `mod` declarations and re-exports only — no `fn`, no
  `struct`, no `impl`.
- The 82-item public surface is unchanged, and **zero files outside
  `crates/cyrup-ext-subagents/src/exec/acceptance/` are modified**.
- The collapse-status banner survives verbatim in `model/mod.rs` — it is the only record of what
  remains to be collapsed.
- No file exceeds ~1,500 lines.
- Content preservation demonstrated by the line-level comparison, with every residual difference
  explained.
- `cargo check --workspace --all-targets` clean; `cargo test -p cyrup-ext-subagents` at its current
  pass count; `cargo clippy -p cyrup-ext-subagents --all-targets --no-deps -- -D warnings` reporting
  no finding this change introduced; `cargo doc` warning count under `exec/acceptance/` no higher
  than before.

## Follow-on work to queue separately — do NOT do it here

Finishing the C12 collapse: `exec/mod.rs::run_sync` calling `model::evaluate_acceptance`,
`SingleResult.acceptance` becoming `model::AcceptanceLedger`, and `lattice/` being deleted. It is
untracked today and it is a behaviour change; this task is behaviour-preserving and must stay so.
