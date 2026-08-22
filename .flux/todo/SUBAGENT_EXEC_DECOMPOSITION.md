---
stage: aug
status: done
updated: 2026-08-22 14:24
---

# Decompose Subagent Exec Modules Into Submodules

## Description

Decompose the two oversized modules in `cyrup-ext-subagents` into submodules along logical
separation-of-concerns lines:

| lines | file |
| ----: | ---- |
| 9,599 | [`crates/cyrup-ext-subagents/src/exec/acceptance.rs`](../../crates/cyrup-ext-subagents/src/exec/acceptance.rs) |
| 7,926 | [`crates/cyrup-ext-subagents/src/exec/mod.rs`](../../crates/cyrup-ext-subagents/src/exec/mod.rs) |

This is a **pure relocation refactor**. Every item moves verbatim — same code, same doc comments,
same order within its new home. Nothing is rewritten, renamed, reflowed, merged, or "improved" on
the way across. The only edits permitted are the four mechanical classes named in **Mechanics**
below (module declarations, `use` lines, visibility keywords, and path fixes made necessary by the
move itself).

---

## Why this decomposes cleanly

Both files are already internally sectioned by banner comments and both have their entire test
surface concentrated in one trailing `mod tests`. Verified anatomy:

**`exec/mod.rs`** — 7,926 lines
- `1–38` — `//!` module doc for `exec` (stays put)
- `39–125` — 19 `pub mod` declarations for existing siblings (stay put)
- `127–160` — the `use` block (redistributed)
- `159–4574` — the body: 56 top-level items, listed in **Target layout B**
- `4575–7926` — one `#[cfg(test)] mod tests` (~3,352 lines, 90 test fns)

**`exec/acceptance.rs`** — 9,599 lines. Two stacked layers plus two test blocks:
- `1–69` — `//!` module doc
- `70–74` — the `use` block
- `76–1827` — the **enum-lattice layer**: `AcceptanceStatus` / `AcceptanceContract` /
  `AcceptanceLedger` / `evaluate_acceptance`, the crate's original acceptance subsystem
- `1828–3619` — `#[cfg(test)] mod tests` for that layer (~1,792 lines, 68 test fns)
- `3621–3654` — the `//` banner introducing `model`
- `3655–7820` — `pub mod model { … }`, the faithful pi port (~4,166 lines, 64 `pub` items)
- `7821–9598` — `#[cfg(test)] mod tests` inside `model` (~1,778 lines, 40 test fns)
- `9599` — closing `}` of `pub mod model`

The two layers are already documented as distinct (`acceptance.rs:3621-3654` spells out what has
and has not collapsed between them), which is exactly the seam to cut on.

---

## Invariants — non-negotiable

1. **The public API does not change.** Every path that resolves today must resolve after.
   Confirmed consumers across the workspace:
   - `cyrup_ext_subagents::exec::{run_sync, plan_batch, AgentConfig, RunOptions, SingleResult,
     AgentProgress, ResolvedAgentPersona, resolve_step_agent_config, LiveEventSink,
     ToolCallSummary, build_attempt_spawn_plan, build_attempt_spawn_plan_with_read_requirement,
     AttemptSpawnPlan, BatchForkRequest, apply_thinking_suffix, split_known_thinking_suffix,
     format_timeout_message, RECENT_OUTPUT_CAP, RECENT_OUTPUT_TAIL_LINES, RECENT_OUTPUT_LINE_CHARS,
     PARENT_SESSION_ENV_VAR, AGENT_NAME_ENV_VAR, ProgressSnapshotInput, RunModelOverride,
     RunModelAttempt}`
   - `…::exec::acceptance::{AcceptanceStatus, AcceptanceContract, AcceptanceLedger,
     AcceptanceFileOutput, CleanCompletionGate, ReviewerResult, PostHocCorrection, VerifyCommand,
     ACCEPTANCE_REJECTED_EXIT_CODE, lower_acceptance_input, inject_acceptance_contract,
     build_timed_out_acceptance_ledger, run_verify_commands, run_verify_commands_memoized,
     run_verify_commands_memoized_with_cancel, evaluate_acceptance, evaluate_acceptance_with_cancel,
     apply_post_hoc_correction, model}`
   - `…::exec::acceptance::model::{validate_acceptance_input, strip_acceptance_report,
     run_verify_command, run_memoized_verify_command, aggregate_acceptance_report, AggregateChild,
     …}` — all 64 `pub` items in `model`
   - `crate::lib.rs:81` re-exports `exec::{AGENT_NAME_ENV_VAR, PARENT_SESSION_ENV_VAR}` at the crate
     root.
   Callers live in `extension.rs`, `spawn/chain_graph.rs`, `discovery/chains.rs`,
   `background/runner_main.rs`, `background/spawn_detached.rs`, `tui/intercom.rs`,
   `registration/slash_commands.rs`, `spawn/signal.rs`, the in-crate `src/tests/`, and
   **`crates/cyrup-it/tests/subagents/*`** (a separate crate — it only sees `pub` paths, so it is
   the sharpest check that the surface held).
2. **No behaviour change.** No function body is edited. In particular
   `build_attempt_spawn_plan_with_read_requirement` (767 lines) and `run_sync` (830 lines) move as
   single indivisible units — do **not** break them into helper functions in this task.
3. **No new public paths.** Modules created to hold what was previously inline in `mod.rs` are
   declared **private** (`mod foo;`) and their contents re-exported (`pub use self::foo::…;`). That
   keeps `exec::run_sync` working without also creating `exec::run::run_sync`. The one exception is
   `acceptance::model`, which is `pub` today and stays `pub`.
4. **Doc comments move verbatim.** These files carry load-bearing citations (`func-SA §5.2`,
   `R-SA-032`, `acceptance.ts:1193`, `@v0.43.0`). Do not reword, re-wrap, shorten, or "tidy" a
   single line of prose. Only intra-doc **link paths** that break may be edited (see M5).
5. **Every test that exists today still exists and still passes**, in the module whose code it
   exercises.

---

## Target layout A — `exec/acceptance.rs` → `exec/acceptance/`

Line ranges are the source anchor. Take each item together with its preceding doc comment,
attributes, and any `// ====` banner that introduces it.

| new file | source lines | contents |
| --- | --- | --- |
| `acceptance/mod.rs` | `1–69`, `3621–3654` | The `//!` module doc verbatim; the `mod`/`pub use` facade; nothing else. The `model` banner comment moves to `model/mod.rs` (M5). |
| `acceptance/status.rs` | `76–186`, `1078–1101` | `AcceptanceStatus` enum + **both** its `impl` blocks (the second one, `as_wire_str`, currently sits 900 lines below the first). |
| `acceptance/ledger.rs` | `175–288` | `AcceptanceLedger`, `default_pending_evidence_status`, `impl AcceptanceLedger`, `build_timed_out_acceptance_ledger`. |
| `acceptance/contract.rs` | `274–735` | `pub type VerifyCommand`, `AcceptanceContract` + `impl`, `clamp_requestable_level`, `ReviewerResult`. |
| `acceptance/lowering.rs` | `685–1015` | `lower_acceptance_input`, `lower_verify_command`, `LoweredAcceptancePolicy` + `impl`, `lower_acceptance_policy`, `lower_criterion`. |
| `acceptance/inject.rs` | `1013–1077` | `ACCEPTANCE_CONTRACT_HEADING` (→ `pub(crate)`), `inject_acceptance_contract`. |
| `acceptance/process.rs` | `1194–1289` | `spawn_pipe_drain`, `drained`, `drained_by`, `shell_command` — **all four → `pub(crate)`**. This is the shared subprocess-plumbing module that dissolves the `super::` coupling described in M3. |
| `acceptance/verify_runner.rs` | `1102–1193` | `run_verify_commands`, `run_verify_commands_memoized`, `run_verify_commands_memoized_with_cancel` — the thin ordered loops over `model::run_memoized_verify_command`. |
| `acceptance/gate.rs` | `1286–1634` | `CleanCompletionGate` + `impl`, `evaluate_acceptance`, `evaluate_acceptance_with_cancel`, `declared_structural_failures`. |
| `acceptance/report_source.rs` | `1626–1745` | `AcceptanceFileOutput`, `select_acceptance_report_source`, `self_report_floor`, `ParsedAcceptanceReport` (the outer one), `extract_acceptance_report`. |
| `acceptance/correction.rs` | `1740–1827` | `PostHocCorrection`, `ACCEPTANCE_REJECTED_EXIT_CODE`, `apply_post_hoc_correction`. |
| `acceptance/test_support.rs` | from `1828–1883` | `#[cfg(test)]` fixtures shared by more than one test module: `vc`, `vc_timeout`, `passed`, `output_tail`, plus `clean_gate` (`2861`) and `no_guard_trigger` (`2870`). |

### `model` → `acceptance/model/`

Flat, prefix-grouped. Do **not** add a third nesting level — the facade chain is already two deep.

| new file | source lines | contents |
| --- | --- | --- |
| `model/mod.rs` | `3621–3679` | The `//` banner comment verbatim, the shared `use` block, the `mod`/`pub use` facade. |
| `model/types.rs` | `3663–4270` | Every enum/struct of the ported data model: `AcceptanceLevel` (+ `level_rank`), `AcceptanceEvidenceKind`, `GateSeverity`, `CriterionInput`, `AcceptanceGate`, `AcceptanceVerifyCommand` (+ its `From`/`PartialEq` impls), `AcceptanceReviewGate`, `ReviewSetting`, `AcceptanceConfig`, `AcceptanceInput`, `ResolvedAcceptanceGate`, `ResolvedAcceptanceConfig`, `CriterionStatus`, `CriterionReport`, `CommandRunResult`, `CommandRunReport`, `AcceptanceReport`, `RuntimeCheckStatus`, `AcceptanceRuntimeCheck`, `VerifyRunStatus`, `AcceptanceVerifyResult` (struct only — its `impl` goes to `verify_exec.rs`), `VerifyWorkspaceKind`, `VerifyWorkspaceState`, `ReviewResultStatus`, `ReviewFindingSeverity`, `ReviewFinding`, `AcceptanceReviewResult`, `AcceptanceEvidenceStatus`, `AcceptanceLedgerStatus` (+ `From`), `AcceptanceLedger`, `SerializableGate` (+ `impl`), `required_evidence_for_level`, `SubagentRunMode`. |
| `model/resolve.rs` | `4271–4616` | `AcceptanceResolveInput`, `InferredLevel`, `infer_level`, `normalize_acceptance_input`, `explicit_acceptance_can_disable`, `normalize_criteria`, `unique_evidence`, `resolve_effective_acceptance`. |
| `model/prompt.rs` | `4617–4759` | `ACCEPTANCE_REPORT_EXAMPLE`, `acceptance_requires_child_report`, `format_acceptance_prompt`. |
| `model/report_fence.rs` | `4760–4917` | `FenceMatch`, `fenced_matches`, `fenced_block_bodies`, `extract_balanced_json`, `parse_report_json`. |
| `model/report_normalize.rs` | `4918–5215` | `ACCEPTANCE_REPORT_WRAPPERS`, `ACCEPTANCE_REPORT_FIELDS`, `CRITERION_REPORT_FIELDS`, `COMMAND_REPORT_FIELDS`, `normalized_token`, `normalize_criterion_status`, `normalize_command_result`, `normalize_criterion_report`, `normalize_command_report`, `NormalizedReportValue`, `normalize_acceptance_report_value`. |
| `model/report_validate.rs` | `5216–5541` | `describe_validation_value`, `push_type_error`, `path_for`, `validate_string_array_field`, `validate_acceptance_report`, `has_generic_acceptance_report_signal`. |
| `model/report_parse.rs` | `5542–5980` | `ParsedAcceptanceReport`, `ACCEPTANCE_REPORT_NOT_FOUND`, `ACCEPTANCE_REPORT_FENCE_TAGS`, `parse_acceptance_report_body`, `parse_unterminated_acceptance_report_fence`, `has_acceptance_report_fence_tag`, `find_acceptance_report_fence_opener`, `parse_generic_json_acceptance_report_body`, `parse_acceptance_report`, `parse_acceptance_report_sources`, `find_acceptance_report_marker`, `strip_acceptance_report`, `strip_trailing_acceptance_report_fence`, `strip_trailing_acceptance_marker`, `strip_acceptance_report_from_message_text`. |
| `model/criteria.rs` | `5981–6170` | `report_evidence_present`, `check_criteria_satisfied`, `criterion_status_str`, `check_no_staged_files`, `run_structural_checks`. |
| `model/aggregate.rs` | `6171–6383` | `AggregateChild`, `trim_output`, `unique_strings`, `aggregate_acceptance_report`, `ledger_status_str`. |
| `model/verify_env.rs` | `6384–6528` | `SENSITIVE_ENV_KEY_WORDS`, `is_sensitive_env_key`, `effective_verify_env`, `verify_redaction_env`, `redact_verify_env`, `hash_bytes`. |
| `model/verify_memo.rs` | `6529–6874` | `VerifyMemoContext`, `read_verify_workspace_state`, `MEMO_SHAPE_ACCEPTANCE_VERIFY_RESULT`, `MemoIdentity` + `impl`, `run_memoized_verify_command`, `run_memoized_verify_command_with_cancel`. |
| `model/verify_exec.rs` | `6875–7170` | `DEFAULT_VERIFY_TIMEOUT_MS`, `VERIFY_TIMED_OUT_MESSAGE`, `VERIFY_TIMED_OUT_HELD_PIPES_MESSAGE`, `resolve_verify_cwd`, `impl AcceptanceVerifyResult`, `run_verify_command`, `run_verify_command_with_cancel`, `trim_output_after`. |
| `model/evaluate.rs` | `7171–7465` | `EvaluateAcceptanceInput`, `evaluate_acceptance`, `acceptance_failure_message`. |
| `model/validate_input.rs` | `7466–7820` | `VALID_LEVELS`, `EXPLICIT_REVIEWED_UNAVAILABLE`, `VALID_EVIDENCE_KINDS`, `ACCEPTANCE_OBJECT_EXAMPLE`, `acceptance_evidence_help`, `unsupported_evidence_kind_message`, the four `ACCEPTANCE_*_KEYS` consts, `validate_acceptance_input`, `validate_criteria_input`, `validate_verify_input`, `validate_review_input`. |
| `model/test_support.rs` | from `7825–7870`, `8496` | `#[cfg(test)]` fixtures shared across model test modules: `cfg`, `resolve`, `report_value`, `report_text`, `temp_dir`, `attested_policy_requiring_no_report`. |

---

## Target layout B — `exec/mod.rs` → new siblings under `exec/`

`exec/` already holds 19 sibling modules; these join them. No name collides with an existing one.

| new file | source lines | contents |
| --- | --- | --- |
| `exec/mod.rs` | `1–160` | `//!` doc + the 19 existing `pub mod` decls + the 9 new private `mod` decls + the `pub use` facade. **No item definitions, no `use` block of its own beyond what the facade needs.** |
| `exec/config.rs` | `229–514` | `AgentConfig` + `impl`, `ResolvedAgentPersona` + `impl`, `resolve_step_agent_config`. |
| `exec/run_options.rs` | `504–842` | `pub use … ModelOverride as RunModelOverride` (`504–509`), `RunOptions`, `LiveEventSink`, `LiveEventCallback`, `impl Debug for LiveEventSink`, `impl LiveEventSink`. |
| `exec/result.rs` | `193–233`, `821–1010` | `is_false`, `format_timeout_message`, `pub use … ModelAttempt as RunModelAttempt`, `pub use … ToolCallSummary`, `SingleResult`. Keep `SingleResult`'s serde attributes byte-identical: `skip_serializing_if = "crate::exec::is_false"` keeps working because `mod.rs` re-exports `is_false` (M2). |
| `exec/progress.rs` | `159–220`, `1005–1315` | `RECENT_OUTPUT_CAP`, `RECENT_OUTPUT_TAIL_LINES`, `RECENT_OUTPUT_LINE_CHARS`, `bound_output_line`, `AgentProgress` + `impl`, `ProgressSnapshotInput`. |
| `exec/spawn_plan/mod.rs` | `1306–1328`, `1443–2324` | `AttemptSpawnPlan`, `build_attempt_spawn_plan`, `build_attempt_spawn_plan_with_read_requirement`. |
| `exec/spawn_plan/env_keys.rs` | `1342–1396`, `1435–1442` | `INHERIT_PROJECT_CONTEXT_ENV`, `INHERIT_SKILLS_ENV`, `MCP_DIRECT_TOOLS_ENV`, `PARENT_SESSION_ENV_VAR`, `AGENT_NAME_ENV_VAR`, `MCP_DIRECT_TOOLS_NONE_SENTINEL`, `SYSTEM_PROMPT_FLAG`, `APPEND_SYSTEM_PROMPT_FLAG`, `push_unique`. |
| `exec/spawn_plan/thinking.rs` | `1329–1341`, `1390–1434` | `THINKING_LEVELS`, `apply_thinking_suffix`, `split_known_thinking_suffix`. |
| `exec/spawn_plan/task_text.rs` | `2275–2358` | `build_task_text`. |
| `exec/attempt.rs` | `2354–2906` | `SpawnedChildAttemptRunner`, `AttemptRecord`, `impl AttemptRunner for SpawnedChildAttemptRunner<'_>`, both `#[cfg(unix)]` / `#[cfg(not(unix))]` arms of `process_signal_name` (they move as a pair). |
| `exec/drive.rs` | `2905–3587` | `DriveOutcome`, `structured_output_absent`, `FINAL_STOP_GRACE_MS`, `POST_EXIT_DRAIN_MS`, `contact_supervisor_block_prompt`, `is_assistant_message_end`, `message_end_tool_calls`, `message_end_has_tool_call`, `is_sole_structured_output_tool_call`, `drive_attempt`. |
| `exec/run.rs` | `3576–4530` | `resolve_run_acceptance`, `run_sync`, `completion_guard_projection`. |
| `exec/batch.rs` | `4528–4574` | `BatchForkRequest`, `plan_batch`. |
| `exec/test_support.rs` | from `4628–4708` | `#[cfg(test)] pub(crate)` fixtures used by more than one test module: `sample_agent_config`, `base_opts`. |

`spawn_plan/` is a directory rather than a single file because its test surface alone is ~2,300
lines (see the map below); its four code files stay under ~900 lines each.

---

## Mechanics

### M1 — the facade pattern

`exec/mod.rs` after the move (abbreviated; the 19 existing `pub mod` lines and their doc comments
stay exactly where they are):

```rust
//! …existing module doc, lines 1–38, verbatim…

pub mod acceptance;
// …the other 18 existing `pub mod` declarations, unchanged…

mod attempt;
mod batch;
mod config;
mod drive;
mod progress;
mod result;
mod run;
mod run_options;
mod spawn_plan;
#[cfg(test)]
mod test_support;

pub use self::batch::{BatchForkRequest, plan_batch};
pub use self::config::{AgentConfig, ResolvedAgentPersona, resolve_step_agent_config};
pub use self::progress::{
    AgentProgress, ProgressSnapshotInput, RECENT_OUTPUT_CAP, RECENT_OUTPUT_LINE_CHARS,
    RECENT_OUTPUT_TAIL_LINES,
};
pub use self::result::{RunModelAttempt, SingleResult, ToolCallSummary, format_timeout_message};
pub use self::run::run_sync;
pub use self::run_options::{LiveEventSink, RunModelOverride, RunOptions};
pub use self::spawn_plan::env_keys::{AGENT_NAME_ENV_VAR, PARENT_SESSION_ENV_VAR};
pub use self::spawn_plan::thinking::{apply_thinking_suffix, split_known_thinking_suffix};
pub use self::spawn_plan::{
    AttemptSpawnPlan, build_attempt_spawn_plan, build_attempt_spawn_plan_with_read_requirement,
};

pub(crate) use self::result::is_false;
```

`acceptance/mod.rs` follows the same shape, with `pub mod model;` kept public:

```rust
//! …lines 1–69, verbatim…

pub mod model;

mod contract;
mod correction;
mod gate;
mod inject;
mod ledger;
mod lowering;
mod process;
mod report_source;
mod status;
mod verify_runner;
#[cfg(test)]
mod test_support;

pub use self::contract::{AcceptanceContract, ReviewerResult, VerifyCommand};
pub use self::correction::{ACCEPTANCE_REJECTED_EXIT_CODE, PostHocCorrection, apply_post_hoc_correction};
pub use self::gate::{CleanCompletionGate, evaluate_acceptance, evaluate_acceptance_with_cancel};
pub use self::inject::inject_acceptance_contract;
pub use self::ledger::{AcceptanceLedger, build_timed_out_acceptance_ledger};
pub use self::lowering::lower_acceptance_input;
pub use self::report_source::AcceptanceFileOutput;
pub use self::status::AcceptanceStatus;
pub use self::verify_runner::{
    run_verify_commands, run_verify_commands_memoized, run_verify_commands_memoized_with_cancel,
};
```

`model/mod.rs` re-exports all 64 of its `pub` items the same way. Enumerate them explicitly rather
than glob-importing: `cargo doc` renders explicit re-exports as first-class items, and an explicit
list is what catches an accidentally-dropped item at compile time.

### M2 — visibility

Every item that used to be private-within-one-file and now has a caller in a sibling file becomes
**`pub(crate)`** — not `pub`. `pub(crate)` cannot widen the external surface, so it is always the
safe choice here. Known cases:

- `acceptance`: `spawn_pipe_drain`, `drained`, `drained_by`, `shell_command`,
  `ACCEPTANCE_CONTRACT_HEADING`, `clamp_requestable_level`, `lower_verify_command`,
  `lower_acceptance_policy`, `lower_criterion`, `LoweredAcceptancePolicy`,
  `declared_structural_failures`, `select_acceptance_report_source`, `self_report_floor`,
  `extract_acceptance_report`, the outer `ParsedAcceptanceReport`,
  `default_pending_evidence_status`.
- `exec`: `is_false` (already `pub(crate)`; re-exported from `mod.rs` so the serde attribute string
  `crate::exec::is_false` keeps resolving), `bound_output_line`, `push_unique`, `build_task_text`,
  `THINKING_LEVELS` and the env/flag consts, `DriveOutcome`, `AttemptRecord`,
  `SpawnedChildAttemptRunner`, `structured_output_absent`, `process_signal_name`, `drive_attempt`,
  `resolve_run_acceptance`, `completion_guard_projection`, `FINAL_STOP_GRACE_MS`,
  `POST_EXIT_DRAIN_MS`, the four event predicates.
- `model`: anything a sibling model file now calls — e.g. `level_rank`, `required_evidence_for_level`,
  `normalized_token`, `fenced_matches`, `fenced_block_bodies`, `extract_balanced_json`,
  `parse_report_json`, the `normalize_*` family, `validate_acceptance_report`,
  `has_generic_acceptance_report_signal`, `resolve_verify_cwd`, `trim_output_after`,
  `effective_verify_env`, `verify_redaction_env`, `hash_bytes`, `is_sensitive_env_key`,
  `criterion_status_str`, `ledger_status_str`, `trim_output`, `unique_strings`, `MemoIdentity`.

Do **not** promote a private *module* to `pub` to make a path resolve. A private module is visible
to its own descendants: `acceptance/model/prompt.rs` can legally write
`crate::exec::acceptance::inject::ACCEPTANCE_CONTRACT_HEADING` even though `mod inject;` is private,
because `model` is a descendant of `acceptance`. Reach for that before reaching for `pub`.

### M3 — the `super::` sites inside `model` (this is the one real trap)

`model` currently reaches *up* into the enum-lattice layer with `super::`. Once `model` is a
directory under `acceptance/`, `super::` from `model/verify_exec.rs` resolves to `model`, not to
`acceptance` — these break silently into "cannot find" errors, or worse, resolve to the wrong item.
The exact sites, all verified:

| line | today | after |
| ---: | --- | --- |
| `4684` | `super::ACCEPTANCE_CONTRACT_HEADING` | `crate::exec::acceptance::inject::ACCEPTANCE_CONTRACT_HEADING` |
| `5832` | `Option<&super::AcceptanceFileOutput<'_>>` | `Option<&crate::exec::acceptance::AcceptanceFileOutput<'_>>` |
| `6991` | `super::shell_command(…)` | `crate::exec::acceptance::process::shell_command(…)` |
| `7035`, `7036` | `super::spawn_pipe_drain` | `crate::exec::acceptance::process::spawn_pipe_drain` |
| `7068`, `7096` | `super::drained_by(…)` | `crate::exec::acceptance::process::drained_by(…)` |
| `7069`, `7097` | `super::TIMEOUT_SIGTERM_GRACE` | `crate::spawn::signal::TIMEOUT_SIGTERM_GRACE` (import it directly — the outer file's `use` at line `74` was the only reason `super::` worked) |
| `7179` | `Option<super::AcceptanceFileOutput<'a>>` | `Option<crate::exec::acceptance::AcceptanceFileOutput<'a>>` |
| `8289`, `8352`, `8374`, `8393`, `8408` (tests) | `super::super::AcceptanceFileOutput` | `crate::exec::acceptance::AcceptanceFileOutput` |

Creating `acceptance/process.rs` is what makes the first group honest: the four subprocess-plumbing
helpers stop being "private helpers of the legacy layer that `model` happens to borrow" and become a
named shared concern with two declared consumers (`verify_runner.rs` and `model/verify_exec.rs`).

After the move, `rg 'super::' crates/cyrup-ext-subagents/src/exec/acceptance/` must return only
`use super::*;` lines inside `mod tests` blocks. Anything else is an unaudited upward reach.

### M4 — test relocation

Each destination file ends with its own inline test module, carrying the **same inner attribute
block** the source used (they are load-bearing — the crate `#![deny]`s `unwrap_used`, `expect_used`,
`panic`, `indexing_slicing` at `lib.rs:20-24`):

```rust
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    // `exec/mod.rs`'s block additionally carries `clippy::panic` — keep it wherever a moved
    // test uses `panic!` or `unwrap_or_else(|| panic!(…))`, and keep the comment above it verbatim.

    use super::*;
    // plus whatever the moved tests actually name
}
```

Fixtures used by tests in more than one destination go to `test_support.rs`
(`#[cfg(test)] pub(crate)`), imported as `use crate::exec::test_support::*;`. Fixtures used by
exactly one destination move inline with their tests (e.g. `delivered_system_prompt` at `5317` and
`read_system_prompt_arg` at `7463` both belong to the spawn-plan persona tests).

`spawn_plan`'s tests are too large for one file; give it `spawn_plan/tests/` with
`mod.rs` + facet files. A `#[cfg(test)] mod tests;` child of `spawn_plan` can still see
`spawn_plan`'s private items — privacy admits descendants — so no visibility change is needed just
to host them.

### M5 — comments, doc links, and citations

- The `//` banner at `acceptance.rs:3621-3654` moves to the top of `model/mod.rs` **as `//`
  comments**. Do not convert it to `//!`; it contains prose that has never been rustdoc-parsed.
- Intra-doc links break when the target's path changes — e.g. `[`model::check_criteria_satisfied`]`
  written from the old outer scope. Repair by **path only**: prefer the fully-qualified
  `[`crate::exec::acceptance::model::check_criteria_satisfied`]`. `cargo doc` is the oracle here.
- Prose citations naming a source file must be re-pointed. Today 58 in-crate comments and 3
  line-anchored ones say `exec/mod.rs::…` or `exec/acceptance.rs:NNNN`; the line-anchored trio is
  `missions/lifecycle.rs:652` (`exec/mod.rs:759`), `extension.rs:6752` (`exec/mod.rs:3650`), and
  `cyrup-session-svc/src/builder.rs:2106` (`exec/mod.rs:1495-1499`). Update the **path** in each
  reference whose named item moved (`exec/mod.rs::run_sync` → `exec/run.rs::run_sync`); for the
  three line-anchored ones, drop the stale `:NNNN` and cite the item by name in its new file. Change
  nothing else in those sentences.
- `docs/` carries 64 more such references. Those are historical audit records — **leave them
  alone**; they document what the tree looked like when the audit ran.

---

## Test relocation map

### `exec/mod.rs` tests (`4575–7926`, 90 test fns)

| source lines | destination |
| --- | --- |
| `4589–4627` `a_crashed_child_reports_the_posix_signal_name_not_a_number` | `attempt.rs` |
| `4628–4708` `sample_agent_config`, `base_opts` | `exec/test_support.rs` |
| `4709–4779` capability-ceiling → child env | `spawn_plan/tests/env_overlay.rs` |
| `4780–4969` `record_event_*`, `recent_output_*`, `append_recent_output_*`, `summarized_tool_calls_*` | `progress.rs` |
| `4970–5316` `build_task_text_*`, default-reads, acceptance-contract/output-path injection | `spawn_plan/task_text.rs` |
| `5317–5934` persona spill file, system-prompt delivery, refinement overlay, tools allowlist, tool-diagnostic handshake, `require_read_tool_*` | `spawn_plan/tests/persona.rs` + `spawn_plan/tests/tools.rs` |
| `5935–6268` memory scope, tool budget, steer inbox, detached-runner handoff | `spawn_plan/tests/env_overlay.rs` |
| `6269–6389` output-schema env vars | `spawn_plan/tests/schema.rs` |
| `6390–6702` append/replace mode, output-path override composition | `spawn_plan/tests/persona.rs` |
| `6703–6929` tools flag, intercom bridge, supervisor channel, identity markers | `spawn_plan/tests/env_overlay.rs` |
| `6930–7027` session flag / session dir / share | `spawn_plan/tests/session.rs` |
| `7028–7194` depth envelope, subagent-child marker, fanout authorization | `spawn_plan/tests/env_overlay.rs` |
| `7195–7253` `apply_thinking_suffix_*` | `spawn_plan/thinking.rs` |
| `7254–7617` model suffix, inherited session model, skills, mcp refs, extensions, depth increment | `spawn_plan/tests/env_overlay.rs` |
| `7618–7703` `resolved_agent_persona_round_trips_*`, `to_agent_config_stamps_*` | `config.rs` |
| `7704–7861` `run_sync_*` | `run.rs` |
| `7862–7926` `plan_batch_*` | `batch.rs` |

### `acceptance.rs` outer tests (`1828–3619`)

| source lines | destination |
| --- | --- |
| `1832–1883` imports + `vc`/`passed`/`output_tail`/`vc_timeout` | `acceptance/test_support.rs` |
| `1884–1929` lattice ordering, `satisfies`, serde round-trip | `status.rs` |
| `1930–2212` `heuristic_default_*`, explicit/inferred level interaction, clamping, disabling forms, `with_reviewer_result` | `contract.rs` |
| `2213–2265` contract-block injection into task text | `inject.rs` |
| `2266–2313` `a_reviewer_gated_contract_projects_onto_checked_never_onto_verified` | `contract.rs` |
| `2314–2651` real-subprocess verify behaviour: exit codes, output tail, cwd, timeout kill, process group, SIGTERM→SIGKILL, daemonize, cancellation | `model/verify_exec.rs` (these drive `model::run_verify_command` through the `run_one_verify_command` alias declared at `1839`) |
| `2652–2675` `run_verify_commands_executes_every_command_in_order_and_never_short_circuits` | `verify_runner.rs` |
| `2676–2860` `lower_acceptance_input_carries_every_declared_verify_field`, declared cwd/env/timeout/allowFailure | `lowering.rs` |
| `2861–2878` `clean_gate`, `no_guard_trigger` | `acceptance/test_support.rs` |
| `2879–3119` `evaluate_acceptance` gate behaviour, reviewer levels, live aliased child report | `gate.rs` |
| `3120–3285` reviewed-as-level rejection, advertised levels, wire-form rejection | `lowering.rs` |
| `3286–3322` `self_report_floor_distinguishes_claimed_from_attested` | `report_source.rs` |
| `3323–3437` post-hoc correction | `correction.rs` |
| `3438–3619` report-source precedence + truth table | `report_source.rs` |

### `model` tests (`7821–9598`)

| source lines | destination |
| --- | --- |
| `7825–7870` `cfg`, `resolve`, `report_value`, `report_text`, `temp_dir` | `model/test_support.rs` |
| `7871–7974` inference + explicit strengthening/disabling | `model/resolve.rs` |
| `7975–7998` `formats_a_standardized_child_prompt_section` | `model/prompt.rs` |
| `7999–8066` fence parsing, json-family fences, wrapper unwrapping, generic-json non-strip | `model/report_parse.rs` |
| `8067–8100` `reports_field_level_validation_errors` | `model/report_validate.rs` |
| `8101–8178` alias/singleton normalization, duplicate ids, unsupported fields | `model/report_normalize.rs` |
| `8179–8341` wrapper spellings, unterminated fence recovery, marker path | `model/report_parse.rs` |
| `8342–8438` `report_sources_prefer_the_file_*` | `model/report_parse.rs` |
| `8439–8495` `a_daemonizing_verify_command_times_out_*` | `model/verify_exec.rs` |
| `8496–8519` `attested_policy_requiring_no_report` | `model/test_support.rs` |
| `8520–9394` report-optional evaluation, checked/verified/review gates, aggregate evidence, dynamic-fanout parking | `model/evaluate.rs` |
| `8726–8770` `duplicate_normalized_criterion_ids_are_rejected` | `model/report_normalize.rs` |
| `9395–9494` status-token normalization, `not-run`/`not-applicable` alias folding | `model/report_normalize.rs` |
| `9495–9559` `a_blank_evidence_entry_is_not_admissible_evidence` | `model/criteria.rs` |
| `9560–9598` `validates_invalid_disable_and_verify_shapes` | `model/validate_input.rs` |

Where a range's boundary lands mid-test, the test function is the atom — take the whole `fn` with
its doc comment.

---

## Execution order

Work in six commits, each of which compiles and passes the crate's tests on its own. Never leave the
tree broken between commits.

1. **`acceptance/` skeleton.** Create the directory, move the header doc into `acceptance/mod.rs`,
   move the enum-lattice layer (`76–1827`) into its ten files, wire the facade, apply M2/M3, delete
   nothing yet from `model`. Move `model` across wholesale as a single `acceptance/model.rs` in this
   step to keep the tree valid. Move the outer test block per the map.
2. **`acceptance/model/` split.** Break `acceptance/model.rs` into its fifteen files + facade. Apply
   M3's rewrites. Move the model test block per the map.
3. **`exec/` leaf types.** `config.rs`, `run_options.rs`, `result.rs`, `progress.rs`, `batch.rs`
   with their tests; add the facade re-exports.
4. **`exec/spawn_plan/`.** The four code files + the `tests/` subdirectory.
5. **`exec/` execution path.** `attempt.rs`, `drive.rs`, `run.rs` with their tests. `exec/mod.rs`
   should now contain only doc, `mod` declarations and `pub use`.
6. **Citation sweep.** M5's path re-pointing across the 58 + 3 in-crate references.

After each commit:

```bash
cargo check -p cyrup-ext-subagents
cargo clippy -p cyrup-ext-subagents --all-targets
cargo test  -p cyrup-ext-subagents
```

And once, at the end — `cyrup-it` is a separate crate and only sees `pub` paths, so it is the real
proof the surface held:

```bash
cargo check -p cyrup-it --tests
cargo doc   -p cyrup-ext-subagents --no-deps
```

---

## Definition of done

- [ ] `exec/acceptance.rs` is gone, replaced by `exec/acceptance/` (11 code files + facade) and
      `exec/acceptance/model/` (15 code files + facade); no file in either exceeds ~1,200 lines
      including its tests.
- [ ] `exec/mod.rs` contains only its `//!` doc, module declarations and re-exports — no `struct`,
      `enum`, `fn`, `const` or `impl` definitions remain in it.
- [ ] Every public path listed under **Invariants #1** resolves unchanged; `cargo check -p cyrup-it
      --tests` compiles without a single import edit in that crate.
- [ ] No function body was edited. `git diff -M -C --stat` on the refactor commits shows
      moves/renames, not rewrites; a spot-check of `run_sync` and
      `build_attempt_spawn_plan_with_read_requirement` shows byte-identical bodies.
- [ ] `rg 'super::' crates/cyrup-ext-subagents/src/exec/acceptance/` returns only `use super::*;`
      inside test modules.
- [ ] `cargo check`, `cargo clippy --all-targets` and `cargo test` are clean for
      `cyrup-ext-subagents` with **no new warnings** versus the pre-refactor baseline (capture the
      baseline before starting).
- [ ] `cargo doc -p cyrup-ext-subagents --no-deps` emits no broken-intra-doc-link warning that was
      not already present before the refactor.
- [ ] Test count is unchanged: **90** in the old `exec/mod.rs` block, **68** in the acceptance outer
      block, **40** in the `model` block — 198 in total, all present, all passing, none skipped,
      `#[ignore]`d or deleted. `cargo test -p cyrup-ext-subagents` reports the same pass count as the
      pre-refactor baseline.
