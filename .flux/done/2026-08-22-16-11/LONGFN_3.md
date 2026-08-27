---
stage: qa
status: completed
updated: 2026-08-27 07:41
---

# Decompose run_attempt (411) And drive_attempt (475)

> Split from `DECOMPOSE_LONG_FUNCTIONS` (3 of 7). That task re-measured the crate and found
> **10 non-test functions over 400 lines** — unchanged from the original survey, because the ~30
> merged decomposition PRs split *files*, not *functions*. `exec/mod.rs` went 4,575 → 1,443 lines
> while every long function travelled at full length.

## OBJECTIVE

Decompose the two attempt-lifecycle functions. Both encode ordering that a reorder would silently break: `run_attempt`'s first-error-wins cascade and `drive_attempt`'s `biased;` select. `drive_attempt`'s single `ChildStep::Line(Ok(line))` arm is 206 lines — 43% of the function — and is the primary extraction.

| function | location | now | target |
|---|---|---:|---:|
| `run_attempt` | [`crates/cyrup-ext-subagents/src/exec/attempt_runner.rs:95`](../../crates/cyrup-ext-subagents/src/exec/attempt_runner.rs) | 411 | ≤120 |
| `drive_attempt` | [`crates/cyrup-ext-subagents/src/exec/drive_attempt.rs:174`](../../crates/cyrup-ext-subagents/src/exec/drive_attempt.rs) | 475 | ≤150 |

Total to decompose in this task: **886 lines**.

## Subtasks

### SUBTASK1 — Extract `run_attempt`'s phases, preserving the first-error-wins cascade order

**What / where / why:** see §3a in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK2 — Extract `drive_attempt`'s 206-line `ChildStep::Line(Ok(line))` arm into its own function

**What / where / why:** see §3b in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK3 — Extract `drive_attempt`'s remaining arms, preserving the `biased;` select order

**What / where / why:** see §3c in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

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

## §3 — `run_attempt` (411 → target ≤ 120)

[`exec/attempt_runner.rs:95-505`](../../crates/cyrup-ext-subagents/src/exec/attempt_runner.rs).
Trait-impl method on `SpawnedChildAttemptRunner`. Three clean phases plus a diagnosis cascade.

- **Prepare (100–243).** `AgentProgress` init (100), attempt-note push (110), `ControlMonitor::new`
  (128), `task_text` (143), `child_depth` (159), `build_attempt_spawn_plan_with_read_requirement`
  (162, with its failure arm through 194), `jsonl_path` + `attempt_index += 1` (195),
  `tool_diagnostic_path` (210), `SpawnedChild::spawn` with its failure arm (215–243). Extract as
  `fn prepare_attempt(&mut self, model, attempt_note) -> Result<PreparedAttempt, (AttemptSignal, AttemptRecord)>`
  where `PreparedAttempt { child, progress, control, jsonl_path, tool_diagnostic_path }` and the
  `Err` arm carries the already-built failure pair from either the plan error or the spawn error.
- **Drive (244–257).** `take_stderr` (244), `deadline_sleep` (246), `drive_attempt(..)` (250). Leave
  inline — three statements.
- **Diagnose (258–457).** This is the real bulk and the real value. `outcome.interrupted` (258), the
  `(raw_exit_code, spawn_error, process_signal)` destructure (280), `final_output` (286), the
  timeout branch (291), then a **five-step `if error.is_none()` cascade** at **336, 342, 352, 357**
  seeded by `let mut error = match outcome.turn_budget.terminal_note()` (330), followed by
  `forced_drain_after_final_success` (368), the non-zero-exit arm (379), the `exit_code` derivation
  (390), the `error.is_some() && exit_code == 0` correction (399), and two `exit_code == 0 && ..`
  cold-start checks (405, 432). Extract as

  ```rust
  /// The ordered diagnosis cascade: the FIRST condition that fires owns the error string, and
  /// every later check is a no-op. Order is the contract — see each arm's comment.
  fn diagnose_attempt(
      outcome: &DriveOutcome,
      raw_exit_code: Option<i32>,
      spawn_error: Option<String>,
      final_output: Option<&str>,
      progress: &AgentProgress,
      structured_requested: bool,
  ) -> AttemptDiagnosis   // { exit_code: i32, error: Option<String>, success: bool }
  ```

  Turning the `if error.is_none()` chain into an `Option::or_else` chain (or a sequence of early
  returns) makes "first wins" structural rather than conventional.
- **Assemble (446–504).** `success` (446), `error_is_placeholder` (455), the returned
  `(AttemptSignal, AttemptRecord)` tuple (459–504). Leave inline.

---

## §4 — `drive_attempt` (475 → target ≤ 150)

[`exec/drive_attempt.rs:174-648`](../../crates/cyrup-ext-subagents/src/exec/drive_attempt.rs).
Structure: setup **175–219** → `loop {` at **220** → `match child.wait_final_drain().await` at
**603** → end **648**.

Inside the loop: three inline `async` arms (`deadline_arm` at **221**, `final_drain_arm` at **230**,
`exit_drain_arm` at **239**), then `tokio::select!` at **246** with arms `cancel.cancelled()`
(**247**), `interrupt.cancelled()` (**261**), `deadline_arm` (**280**), and
`step = child.next_event_or_exit()` (**300**).

**The whole problem is one arm.** `step = child.next_event_or_exit()` runs **300–588**, and inside
it the `match step` at **301** has four arms of which `ChildStep::Line(Ok(line))` is **302–508** —
206 lines, 43% of the function. Extract exactly that:

```rust
/// One NDJSON line from the child: parse, fold into `progress`, update the turn budget, feed the
/// control monitor. Returns what the drive loop should do next.
enum LineAction { Continue, Detached, Settled, Break }

fn handle_child_line(
    line: &str,
    progress: &mut AgentProgress,
    control: &mut crate::exec::control::ControlMonitor,
    turn_budget: &mut crate::exec::turn_budget::TurnBudgetTracker,
    agent_settled: &mut bool,
    detached_seen: &mut bool,
    ..
) -> LineAction
```

Then, if any of the three shorter arms exceeds ~20 lines, give it its own helper too:
`ChildStep::ProtocolLimit` (**509–531**), `ChildStep::Line(Err(_)) | ChildStep::Eof` (**533–537**),
`ChildStep::Exited(_)` (**538–**). The `activity_tick` handling at **589–599** and the
`wait_final_drain` match at **603–647** stay inline.

Do **not** try to lift the `tokio::select!` itself into a helper — the arms borrow `child`,
`progress` and `control` simultaneously and the biased ordering (`biased;` at **247**) is
load-bearing.

---
