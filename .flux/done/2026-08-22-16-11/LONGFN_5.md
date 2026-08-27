---
stage: qa
status: completed
updated: 2026-08-27 08:34
---

# Decompose runner_main.rs run_single (404 lines)

> Split from `DECOMPOSE_LONG_FUNCTIONS` (5 of 7). That task re-measured the crate and found
> **10 non-test functions over 400 lines** — unchanged from the original survey, because the ~30
> merged decomposition PRs split *files*, not *functions*. `exec/mod.rs` went 4,575 → 1,443 lines
> while every long function travelled at full length.

## OBJECTIVE

Reduce `run_single` from 404 lines to under 120. Its three 113–148-line `RunOptions` struct literals are the bulk and are the primary extraction target.

| function | location | now | target |
|---|---|---:|---:|
| `run_single` | [`crates/cyrup-ext-subagents/src/background/runner_main.rs:2375`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | 404 | ≤120 |

Total to decompose in this task: **404 lines**.

## Subtasks

### SUBTASK1 — Collapse the three large `RunOptions` struct literals into a shared builder or helper

**What / where / why:** see §5a in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK2 — Extract the remaining phases so the parent is under 120 lines

**What / where / why:** see §5b in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

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

## §7 — `run_single` in `runner_main.rs` (404 → target ≤ 120)

[`background/runner_main.rs:2375-2778`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs).
Dominated by one 148-line struct literal.

| lines | phase | extract as |
|------:|-------|-----------|
| 2392–2489 | persona lookup (2392), `to_agent_config` (2398), step `tools` override (2402), `max_depth_override` (2408), `available_models` (2422), `resolve_model_inheritance` (2434), `acceptance` (2471) | `fn build_step_agent_config(&self, step) -> Result<(AgentConfig, Option<ModelId>, Option<AcceptanceContract>), SubagentError>` |
| 2490–2532 | `interrupt_token` (2490), the already-interrupted early return (2491), `live_events` (2502), `fork_context` (2511), `effective_cwd` (2519), `output_path` (2525) | leave inline |
| **2533–2680** | the `RunOptions { .. }` literal — **148 lines** | `fn build_step_run_options(&self, step, ctx, agent, ..) -> RunOptions` |
| 2698–2714 | `artifact_paths` (2698) | leave inline |
| 2715 | `exec::run_sync` | leave inline |
| 2722–2745 | artifact writing (2722) | `fn write_step_artifacts(paths, run_token, result: &SingleResult)` |
| 2746–2777 | `step_result` assembly (2746) + the field copies | leave inline |

The `RunOptions` literal is where the function's 221 comment lines mostly live (upstream `pi`
citations); moving it to its own `fn` with a doc comment naming what it maps (`SingleStepSpec` +
`ChainRunContext` → `RunOptions`) is most of the win here.

---
