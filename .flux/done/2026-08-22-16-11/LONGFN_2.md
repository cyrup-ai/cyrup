---
stage: qa
status: completed
updated: 2026-08-27 07:14
---

# Decompose build_attempt_spawn_plan_with_read_requirement (716 lines)

> Split from `DECOMPOSE_LONG_FUNCTIONS` (2 of 7). That task re-measured the crate and found
> **10 non-test functions over 400 lines** — unchanged from the original survey, because the ~30
> merged decomposition PRs split *files*, not *functions*. `exec/mod.rs` went 4,575 → 1,443 lines
> while every long function travelled at full length.

## OBJECTIVE

Reduce `build_attempt_spawn_plan_with_read_requirement` from 716 lines to under 150 by lifting each environment-overlay group into its own named function.

| function | location | now | target |
|---|---|---:|---:|
| `build_attempt_spawn_plan_with_read_requirement` | [`crates/cyrup-ext-subagents/src/exec/spawn_plan.rs:285`](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) | 716 | ≤150 |

Total to decompose in this task: **716 lines**.

## Subtasks

### SUBTASK1 — Separate each env-overlay group into its own function, per the section below

**What / where / why:** see §2a in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK2 — Reduce the parent to a sequence of named overlay calls under 150 lines

**What / where / why:** see §2b in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

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

## §2 — `build_attempt_spawn_plan_with_read_requirement` (716 → target ≤ 150)

[`exec/spawn_plan.rs:285-1000`](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs). This one
is the easiest of the eleven: it is straight-line accumulation into three collections (`args`,
`env_overlay`, `extension_paths`) with no shared mutable state between the groups. Verified seams:

| lines | phase | extract as |
|------:|-------|-----------|
| 309–317 | capability-ceiling preflight (`resolve_current_capability_ceiling` + `assert_agent_allowed`) | `fn preflight_capability_ceiling(agent, opts) -> Result<(), SubagentError>` |
| 319–341 | spawn command, `model_arg` via `apply_thinking_suffix`, base `args` vec | leave inline |
| 343–470 | tool resolution: `required_child_tools` / `effective_mcp_tools` / `builtin_tools` / `tool_extension_paths` / `mcp_direct_tools` (343–367), `fanout_authorized` (368), `explicit_tool_allowlist` (397) and its 74-line body | `fn resolve_child_tools(agent, require_read_tool) -> ResolvedChildTools` |
| 472–510 | extension paths: `tool_extension_paths` + `agent.extensions` merge (476), `--no-skills` on `!agent.inherit_skills` (498) | `fn collect_extension_args(agent, tools: &ResolvedChildTools, args: &mut Vec<String>)` |
| 512–625 | persona composition: memory injection (512) → refinement overlay (537) → output-path prompt (562) → turn-budget prompt (577) → temp-file spill (583) | `fn compose_persona(agent, opts, temp_dir) -> Result<(String, Option<PathBuf>), SubagentError>` |
| 627–647 | fork-context session file (627), `resolve_task_arg` (647) | leave inline |
| 649–978 | **the env overlay** — see below | three helpers |
| 980–999 | `cwd` (980) + the `AttemptSpawnPlan` literal (982) | leave inline |

The env overlay is 330 lines with natural per-group boundaries already marked by the code: depth
envelope (**649**), nested-events child role (**668**), `CYRUP_SUBAGENT_RUN` (**673**),
`PI_CODING_AGENT` (**688**), `AI_AGENT` (**698**), agent identity (**703**), the run anchor
(**730**), the orchestration target + run id (**785**), the structured-output runtime (**850**), two
encoded blobs (**867**, **886**), the tool diagnostic path (**899**), and the three steer paths —
inbox (**943**), capability (**959**), ack dir (**969**). Split by lifetime of the value, not by
line count:

- `fn env_identity_and_depth(agent, depth, fanout_authorized, env: &mut BTreeMap<..>)` — 649–729
- `fn env_orchestration(agent, opts, structured_runtime, env: &mut BTreeMap<..>)` — 730–898 (anchor,
  orch target/run id, structured runtime, the two encoded blobs)
- `fn env_control_channels(opts, tool_diagnostic_path, env: &mut BTreeMap<..>)` — 899–978

Each takes `&mut` on the map so insertion order stays exactly as it is today. Do **not** reorder
insertions while extracting — several of these keys are read by the child's own boot path and the
diff must stay purely mechanical.

If `spawn_plan.rs` (3,830 lines) grows past ~4,000 with these helpers, promote it to a module
directory following the `exec/` idiom: `spawn_plan/mod.rs` (imports + `AttemptSpawnPlan` +
`ResolvedChildTools`), `spawn_plan/tools.rs`, `spawn_plan/persona.rs`, `spawn_plan/env.rs`, each
opening `use super::*;`.

---
