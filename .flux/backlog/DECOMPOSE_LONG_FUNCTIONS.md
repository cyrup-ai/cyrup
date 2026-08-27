---
stage: new
status: done
updated: 2026-08-23 02:15
---

# Break Up The Ten Non-Test Functions Over 400 Lines

## Description

Ten non-test functions exceed 400 lines and 25 exceed 200, out of 3,133 total. They are not comment-inflated: `run_sync` is 530 lines of actual code across its 823 physical lines, binds 52 locals and nests to 24 spaces of indentation.

The concentration is what makes this expensive. Four of the ten live in `src/exec/mod.rs` and together account for 2,428 lines — 53% of that file's 4,575 production lines. `run_sync` describes itself at exec/mod.rs:3656-3658 as 'the sole chokepoint every production spawn path in this crate funnels through (the foreground single-run tool dispatch, the background hop-2 runner's per-step loop, and — via `chain_graph::walk_chain`/`spawn::parallel::run_bounded`'s `SingleStepExecutor` seam — every chain step, parallel fan-out child, and dynamic fan-out child as well)'. So any change to fallback, budgets, acceptance or artifacts means reading 530 lines of straight-line code and 52 live bindings to establish the ordering constraints first, and no phase of it can be unit-tested in isolation. `background/runner_main.rs` shows the same shape independently: `run` + `run_inner` + `run_single` = 1,336 of its 3,337 production lines, with `run_inner` being the entire detached-runner turn loop.

## Evidence

Measured with a brace-matcher over src/ with string and comment contents blanked and `#[cfg(test)]` regions excluded. Longest non-test functions: 823 src/exec/mod.rs:3653 `run_sync`; 716 src/exec/mod.rs:1558 `build_attempt_spawn_plan_with_read_requirement`; 529 src/background/runner_main.rs:1201 `run_inner`; 478 src/exec/mod.rs:3093 `drive_attempt`; 458 src/extension/host/slash.rs:227 `dispatch_slash`; 445 src/extension/executor/foreground.rs:123 `run_foreground_impl`; 438 src/extension/tool/routing.rs:241 `route_single`; 411 src/exec/mod.rs:2419 `run_attempt`; 404 src/background/runner_main.rs:2375 `run_single`; 403 src/background/runner_main.rs:513 `run`; 370 src/spawn/chain_graph.rs:1426 `walk_chain`. Distribution: 10 over 400, 25 over 200, 45 over 150, out of 3,133 non-test functions. Over run_sync's body (exec/mod.rs:3653-4475) awk gives 530 code / 262 comment / 31 blank lines, and `grep -c '^\s*let '` gives 52. The self-description quoted above is verbatim at exec/mod.rs:3656-3658.

## Suggested approach

Do `run_sync` first and separately — it is the highest-leverage one and the riskiest, so it deserves its own commit with the test suite green between each extraction. Peel its phases into named private functions taking one context struct; the 52 locals collapse into that struct's fields, which also makes the ordering constraints explicit instead of implied by line position. `build_attempt_spawn_plan_with_read_requirement` is largely independent argv/env accumulation and splits cleanly per env-var group with no shared mutable state. `runner_main.rs`'s three get the same treatment afterwards. Note the known-untracked C12 acceptance collapse touches `run_sync` too (it should end up calling `model::evaluate_acceptance`); sequencing this task first makes that change land in a small function instead of a 530-line one.

## Acceptance Criteria

- [ ] No non-test function in src/ exceeds 300 lines (re-run the brace-matched measurement; the count over 400 goes from 10 to 0)
- [ ] `run_sync` is under 150 lines and delegates to named private phase functions (depth guard + output-mode validation, worktree/fork-context setup, the model-fallback attempt loop, result assembly) taking an explicit context struct rather than 52 loose bindings
- [ ] At least two of the extracted phases have their own unit tests that do not spawn a child process
- [ ] `src/exec/mod.rs` production line count drops below 3,500 (from 4,575)
- [ ] `cargo test -p cyrup-ext-subagents` passes with no new failures and `cargo clippy -p cyrup-ext-subagents --all-targets --no-deps -- -D warnings` reports no new diagnostics

## Source

- Identified by the `subagents-hygiene-survey` workflow (13 agents, 21 raw findings, 16 confirmed after adversarial verification).
- Effort: large · survey priority: 3 of 6
