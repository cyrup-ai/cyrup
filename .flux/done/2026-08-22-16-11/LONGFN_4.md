---
stage: qa
status: completed
updated: 2026-08-27 08:16
---

# Decompose runner_main.rs run (403) And run_inner (529)

> Split from `DECOMPOSE_LONG_FUNCTIONS` (4 of 7). That task re-measured the crate and found
> **10 non-test functions over 400 lines** — unchanged from the original survey, because the ~30
> merged decomposition PRs split *files*, not *functions*. `exec/mod.rs` went 4,575 → 1,443 lines
> while every long function travelled at full length.

## OBJECTIVE

Decompose the detached-runner entry point and its turn loop. `run_inner` IS the turn loop, and encodes the G77 stop-before-timeout-before-interrupt ordering — that order is load-bearing and must survive verbatim.

| function | location | now | target |
|---|---|---:|---:|
| `run` | [`crates/cyrup-ext-subagents/src/background/runner_main.rs:513`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | 403 | ≤120 |
| `run_inner` | [`crates/cyrup-ext-subagents/src/background/runner_main.rs:1201`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | 529 | ≤150 |

Total to decompose in this task: **932 lines**.

## Subtasks

### SUBTASK1 — Extract `run`'s setup and teardown phases

**What / where / why:** see §4a in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK2 — Extract `run_inner`'s turn-loop body, preserving the G77 stop → timeout → interrupt order

**What / where / why:** see §4b in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

## Constraints on every extraction

## Constraints on every extraction

- **Mechanical only.** No behavior change, no reordering of side-effecting statements, no
  re-deriving a value that is currently threaded. Several of these functions encode upstream `pi`
  ordering that a reorder would silently break — `run_sync`'s post-guard gate re-derivation (§1e),
  `run_attempt`'s first-error-wins cascade (§3), `drive_attempt`'s `biased;` select (§4), the G77
  stop-before-timeout-before-interrupt order in `run_inner` (§6).
- **Comments travel with their code.** The upstream citations (`pi execution.ts:1241-1258`,
  `subagent-runner.ts:4403-4411`, the R-SA / SUBA / G numbers) are the reason each line is where it
  is. When a block becomes a function, its leading comment becomes that function's `///` doc.
- **Visibility.** Extracted helpers are private (`fn`) in the same module unless a sibling file needs
  them, in which case `pub(super)` and re-bound in the parent `mod.rs`, per the `exec/` idiom.
- **No new tests, benchmarks, or documentation are required by this task.** Verifying the existing
  suite still passes is the bar.

## No tests

**No tests are to be written for this task.** Another team owns tests. The bar is that the existing
suite still passes unchanged. The crate has in-file `#[cfg(test)]` modules in the touched files and
none of them should need editing — if an extraction forces a test edit, the extraction changed
behaviour and is wrong.

## No benchmarks

**No benchmarks are to be written for this task.** Another team owns benchmarks.

## Definition of Done

- [ ] Every function in the table above is at or under its target line count, verified by a
      brace-matched measurement with string/comment contents blanked and `#[cfg(test)]` / `#[test]` /
      `#[tokio::test]` regions excluded
- [ ] Every extracted helper is private (`fn`) in the same module, or `pub(super)` and re-bound in the
      parent `mod.rs` if a sibling needs it
- [ ] Every upstream citation comment travels with the code it annotates; a block that becomes a
      function has its leading comment as that function's `///` doc
- [ ] `cargo test -p cyrup-ext-subagents` passes with no new failures and **no test file edited**
- [ ] `cargo clippy -p cyrup-ext-subagents --all-targets --no-deps -- -D warnings` reports no new diagnostics
- [ ] `cargo test --workspace` passes
- [ ] `git diff --stat` shows no file outside `crates/cyrup-ext-subagents/src/` touched

## Workspace idiom

## Workspace idiom

For splitting an over-long **file** into a module directory this workspace has a settled shape: a
`mod.rs` holding the shared imports and the type definitions, siblings opening with `use super::*;`,
helpers promoted to `pub(super)` and re-bound in `mod.rs`. Declaring the main struct in `mod.rs`
lets siblings read its private fields with no visibility change. Worked examples:
[`cyrup-tui/src/app/`](../../crates/cyrup-tui/src/app/),
[`cyrup-tui/src/transcript/`](../../crates/cyrup-tui/src/transcript/),
[`cyrup-tui/src/editor/`](../../crates/cyrup-tui/src/editor/),
[`cyrup-tui/src/selector/`](../../crates/cyrup-tui/src/selector/).
`exec/` in this crate already follows it.

**That idiom is not what this task needs.** Every function below stays in the file it is in today;
what moves is *body* into private helper `fn`s in the same module. Only §2 (`spawn_plan.rs`, already
3,830 lines) may additionally want new sibling files, and only if the extracted helpers make the
file grow past ~4,000 lines.

---

## Research

## §5 — `run` in `runner_main.rs` (403 → target ≤ 120)

[`background/runner_main.rs:513-915`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs).
Verified seams:

| lines | phase | extract as |
|------:|-------|-----------|
| 514–567 | `read_and_delete_config` (514) + the `match outcome` failure arms (516) | `async fn load_runner_config(config_path) -> Result<RunnerConfig, ..>` |
| 568–588 | `effective_paths` / `run_paths` resolution (568), `ensure_accessible_dir` (583), results dir (584) | `async fn resolve_run_paths(config) -> ..` |
| 589–703 | `RunStatus::queued` (589), session id (600), `chain_step_count` (605), `steps` projection (606), `advance_state(Running)` (612), status write (628), the `#[cfg(unix)]` SIGUSR2 guard (656–657), `create_dir_all` (680) | `async fn publish_initial_status(config, run_paths) -> Result<RunStatus, ..>` |
| 704–762 | the four flags — `interrupted` (704), `interrupt_cancel` (711), `timed_out` (717), `stopped` (721) — plus the three inbox pre-checks (`check_stop_inbox_now` 722, `check_timeout_inbox_now` 731, `check_control_inbox_now` 740), folded into `ControlFlags` (752) | `async fn init_control_flags(run_paths) -> ControlFlags` |
| 763–801 | events writer (763), run-started `append_event` (764), `overall_started_at` (773), `shared_status` (778), `spawn_control_watcher` (785), telemetry channel + task (796) | leave inline — it is the wiring `run_inner` needs and reads as one block |
| 802–815 | the `run_inner` call | leave inline |
| 816–900 | `duration_ms` (818), the `(terminal_state, results, final_error)` `match loop_outcome` (820–900) | `fn settle_loop_outcome(loop_outcome, ..) -> (RunState, Vec<SingleResult>, Option<String>)` |
| 901–914 | `final_status` (901) + `finish_run` (903) | leave inline |

---

## §6 — `run_inner` in `runner_main.rs` (529 → target ≤ 150)

[`background/runner_main.rs:1201-1729`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs).
Setup **1219–1339**, then `loop {` at **1340** running to the end.

**Setup (1219–1339).** `resolve_effective_depth` (1219), the `is_blocked` early return (1238),
`GlobalConcurrencyLimit` (1245), `cancel_root` (1246), `resolved_agents` (1251),
`current_flat_index` (1254), the `ExecSingleStepExecutor` construction (1255–1310), `deadline_at`
(1311), `ChainRunContext` (1315–1339). Extract the executor + context construction as
`fn build_chain_context(config, run_paths, status, flags, telemetry, depth, ..) -> (Arc<dyn SingleStepExecutor>, ChainRunContext)`
— that is 85 of the 121 setup lines and it has exactly one output.

**Loop body (1341–1728).** Verified seams:

| lines | phase | extract as |
|------:|-------|-----------|
| 1354–1387 | stop-flag check (G77 — read first, ahead of the other two) | `fn check_stop(..) -> Option<LoopOutcome>` |
| 1388–1431 | timeout-flag check | `fn check_timeout(..) -> Option<LoopOutcome>` |
| 1432–1460 | interrupt-flag check (`&& cursor < steps.len()`) | `fn check_interrupt(..) -> Option<LoopOutcome>` |
| 1461–1485 | `list_pending_appends` (1461) + splice into `steps` (1462) | `async fn absorb_pending_appends(steps: &mut Vec<RunnerStep>, run_paths) -> Result<(), ..>` |
| 1486–1527 | cursor bounds (1486), `step` clone (1490), `current_flat_index.store` (1497), `write_shared_status` (1507), step-started `append_event` (1510) | leave inline |
| 1528–1612 | the `RunnerStep::ImportAsyncRoot(spec)` special case — 85 lines | `async fn run_import_async_root(spec, ctx, ..) -> Result<.., ..>` |
| 1613–1626 | one-step dispatch through `walk_chain` (1613) | leave inline |
| 1627–1726 | post-step: `interrupted_mid_flight` (1627), `step_duration_ms` (1628), `event_type` (1646), step-finished `append_event` (1653), `results.push` (1665), `write_shared_status` (1667), the interrupt arm (1671–1710), the timeout arm (1711–1726) | `async fn record_step_outcome(step, step_result, ..) -> Option<LoopOutcome>` |
| 1727 | `cursor += 1` | leave inline |

The three flag checks share a shape (load an `AtomicBool`, write a terminal status, append an event,
return a `LoopOutcome`); if they collapse cleanly into one
`fn check_terminal_flags(flags, ..) -> Option<LoopOutcome>` with a three-arm match, prefer that —
but only if the G77 ordering comment at **1213** ("read at the very top of every loop iteration
ahead of the other two") survives as an explicit `stopped` → `timed_out` → `interrupted` sequence in
the body.

---
