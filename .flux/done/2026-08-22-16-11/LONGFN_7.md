---
stage: qa
status: completed
updated: 2026-08-27 09:21
---

# Decompose dispatch_slash (458) And walk_chain (370)

> Split from `DECOMPOSE_LONG_FUNCTIONS` (7 of 7). That task re-measured the crate and found
> **10 non-test functions over 400 lines** — unchanged from the original survey, because the ~30
> merged decomposition PRs split *files*, not *functions*. `exec/mod.rs` went 4,575 → 1,443 lines
> while every long function travelled at full length.

## OBJECTIVE

Decompose the slash-command dispatcher and the chain walker. `dispatch_slash` has the most aggressive target (≤60) because it is a pure dispatch table — every non-trivial arm should delegate to a named method. `walk_chain`'s `DynamicGroup` arm is 264 lines, 71% of the function.

| function | location | now | target |
|---|---|---:|---:|
| `dispatch_slash` | [`crates/cyrup-ext-subagents/src/extension/host/slash.rs:227`](../../crates/cyrup-ext-subagents/src/extension/host/slash.rs) | 458 | ≤60 |
| `walk_chain` | [`crates/cyrup-ext-subagents/src/spawn/chain_graph.rs`](../../crates/cyrup-ext-subagents/src/spawn/chain_graph.rs) | 370 | ≤120 |

Total to decompose in this task: **828 lines**.

## Subtasks

### SUBTASK1 — Move every non-trivial `dispatch_slash` match arm into a named method, leaving a dispatch table under 60 lines

**What / where / why:** see §7a in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK2 — Extract `walk_chain`'s 264-line `DynamicGroup` arm into its own function

**What / where / why:** see §7b in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

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

## §10 — `dispatch_slash` (458 → target ≤ 60)

[`extension/host/slash.rs:227-684`](../../crates/cyrup-ext-subagents/src/extension/host/slash.rs).
The body is one `match command { .. }` at **234** and nothing else. Every non-trivial arm becomes an
`async fn slash_<name>(&self, args, cwd, has_ui) -> Result<String, SubagentError>` method — which is
already the shape three arms use today (`SubagentsDoctor` at **375** and `SubagentCost` at **446**
delegate in one line; `SubagentsFleet` at **382** calls `self.show_fleet(..)`).

Verified arm starts and the extraction for each:

| line | arm | extract as |
|-----:|-----|-----------|
| 248 | `Run` (~125 lines — the largest) | `slash_run` |
| 375 | `SubagentsDoctor` | already delegating — leave |
| 382 | `SubagentsFleet` | already delegating — leave |
| 396 | `SubagentsStop` | `slash_subagents_stop` |
| 419 | `SubagentsGuide` | `slash_subagents_guide` |
| 428 | `SubagentsProfiles` | `slash_subagents_profiles` |
| 441 | `SubagentsLoadProfile` | `slash_load_profile` |
| 446 | `SubagentCost` | already delegating — leave |
| 455 | `Chain` | `slash_chain` |
| 471 | `Parallel` | `slash_parallel` |
| 493 | `RunChain` (~56 lines) | `slash_run_chain` |
| 549 | `SubagentsModels` | `slash_models` |
| 570 | `SubagentsRefreshProviderModels` | `slash_refresh_provider_models` |
| 591 | `SubagentsGenerateProfiles` | `slash_generate_profiles` |
| 606 | `SubagentsCheckProfile` | `slash_check_profile` |
| 620 | `PromptWorkflow` | `slash_prompt_workflow` |
| 657 | `ChainPrompts` | `slash_chain_prompts` |

Put the new methods in the same `impl` block, below `dispatch_slash`, in match-arm order. The
per-arm comments move with their bodies onto the new fn's `///` doc. `slash.rs` is only 996 lines
today, so this stays one file.

---

## §11 — `walk_chain` (370 → target ≤ 120)

[`spawn/chain_graph.rs:1417-1786`](../../crates/cyrup-ext-subagents/src/spawn/chain_graph.rs).
Body is `for (step_index, step) in graph.iter().enumerate()` at **1426** with
`let result = match step` at **1427**. Four arms:

| lines | arm | extract as |
|------:|-----|-----------|
| 1428–1445 | `RunnerStep::SingleStep(spec)` (18 lines) | leave inline |
| 1446–1488 | `RunnerStep::ParallelGroup(spec)` (43 lines) | `async fn run_parallel_group(spec, registry, single, ctx, step_index) -> ..` |
| **1489–1752** | `RunnerStep::DynamicGroup(spec)` — **264 lines, 71% of the function** | `async fn run_dynamic_group(spec, registry, single, ctx, step_index) -> ..` |
| 1753–1782 | `RunnerStep::ImportAsyncRoot(spec)` (30 lines) | leave inline |

Extracting `run_dynamic_group` alone takes `walk_chain` from 370 to ~110. Do that one first and stop
there if the parallel arm resists — it is the only extraction this section strictly needs.

---
