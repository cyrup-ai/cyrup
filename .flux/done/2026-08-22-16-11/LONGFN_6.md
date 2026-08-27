---
stage: qa
status: completed
updated: 2026-08-27 09:01
---

# Decompose route_single (438) And run_foreground_impl (445)

> Split from `DECOMPOSE_LONG_FUNCTIONS` (6 of 7). That task re-measured the crate and found
> **10 non-test functions over 400 lines** — unchanged from the original survey, because the ~30
> merged decomposition PRs split *files*, not *functions*. `exec/mod.rs` went 4,575 → 1,443 lines
> while every long function travelled at full length.

## OBJECTIVE

Decompose the extension routing and foreground-execution entry points. `run_foreground_impl` shares the three large `RunOptions` struct literals with `run_single` (LONGFN_5) — if that task landed first, reuse the helper it introduced rather than writing a second one.

| function | location | now | target |
|---|---|---:|---:|
| `route_single` | [`crates/cyrup-ext-subagents/src/extension/tool/routing.rs:241`](../../crates/cyrup-ext-subagents/src/extension/tool/routing.rs) | 438 | ≤130 |
| `run_foreground_impl` | [`crates/cyrup-ext-subagents/src/extension/executor/foreground.rs:123`](../../crates/cyrup-ext-subagents/src/extension/executor/foreground.rs) | 445 | ≤130 |

Total to decompose in this task: **883 lines**.

## Subtasks

### SUBTASK1 — Extract `route_single`'s dispatch arms into named methods

**What / where / why:** see §6a in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK2 — Extract `run_foreground_impl`'s phases, reusing LONGFN_5's `RunOptions` helper if present

**What / where / why:** see §6b in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

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

## §8 — `route_single` (438 → target ≤ 130)

[`extension/tool/routing.rs:241-678`](../../crates/cyrup-ext-subagents/src/extension/tool/routing.rs).
Three independent extractions, all verified:

| lines | phase | extract as |
|------:|-------|-----------|
| 248–285 | agent guard (248), `task` / `context` / `model` (254–256) | leave inline |
| **286–361** | the `SingleRunOverrides { .. }` literal — 76 lines | `fn single_run_overrides(p: &SubagentToolParams) -> SingleRunOverrides` |
| 362–392 | `single_agent_launch_defaults` (362), `resolve_foreground_timeout` (370), `config_snapshot` (380), `resolve_effective_depth` (381), the `p_with_defaults` rebind (382–392) | leave inline |
| **393–515** | the entire `if p.is_background(&cfg, depth) { .. }` branch — 123 lines, and it `return`s | `async fn route_single_background(&self, p, cwd, overrides, cfg, cancel) -> Result<ToolResult, ToolError>` |
| 516–538 | the foreground `run_foreground_impl` call (516) | leave inline |
| **539–678** | result rendering: `display_output` (539), the `details` block (556–587), the clean-run branch (588–629), detached (630–642), interrupted (643–658), non-zero exit (659–666), `text` (667), `Ok(ToolResult { .. })` (672) | `fn render_single_result(result: &SingleResult, run_id: &RunId, ..) -> ToolResult` |

Land them in that order and the residual `route_single` is a dispatcher of about 45 lines.

---

## §9 — `run_foreground_impl` (445 → target ≤ 130)

[`extension/executor/foreground.rs:123-567`](../../crates/cyrup-ext-subagents/src/extension/executor/foreground.rs).
Same shape as §7: a 113-line `RunOptions` literal plus a long config-resolution prologue.

| lines | phase | extract as |
|------:|-------|-----------|
| 128–150 | request destructure (128), `config_snapshot` (139), `resolve_effective_depth` (140), the `is_blocked` early return (141) | leave inline |
| 151–241 | agent + model scope (151), effective/fork context (156–160), `AgentConfig::from_agent_definition` (162), tool budget (169), turn budget (182), `available_models` (199), model override (205), `resolve_model_inheritance` (229) | `async fn resolve_run_agent(&self, req, cfg, depth) -> Result<ResolvedRunAgent, SubagentError>` |
| 242–327 | `deadline_at` (242), `run_id` (247), `resolve_control_config` (259), `control_notifier` (265), `ArtifactConfig` (272), `resolve_artifacts_dir` (281), `output_base_dir` (292), `resolve_single_output_path` (297), `parse_tool_output_mode` (309), `session_dir` (324) | `fn resolve_run_channels(&self, cfg, req, run_id) -> RunChannels` |
| **328–440** | the `RunOptions { .. }` literal | `fn build_foreground_run_options(agent, channels, req, ..) -> RunOptions` |
| 441–498 | the foreground-controls registration block (448–482), `observe_run` (483) | `async fn register_foreground_control(&self, run_id, run_options: &RunOptions)` |
| 499–566 | `art_paths` (499), `drive_foreground_run_sync` (511), notifier flush (527), the post-run block (528–545), `forget_run` (546), `write_foreground_output_artifacts` (548), `Ok((result, run_id))` (566) | leave inline |

---
