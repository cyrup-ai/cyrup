---
stage: split
status: complete
updated: 2026-08-27 05:03
---

# Break Up The Eleven Non-Test Functions Over 300 Lines In `cyrup-ext-subagents`

## Current state (re-measured 2026-08-27, source tree level with `origin/main`)

**The work is still entirely open.** Nothing in this task has been done. The recent decomposition
PRs split *files*, not *functions*: `crates/cyrup-ext-subagents/src/exec/mod.rs` went from 4,575
lines to **1,443** by moving code out into an `exec/` module directory (26 siblings, including
`spawn_plan.rs`, `drive_attempt.rs`, `attempt_runner.rs`, `fallback.rs`, `output.rs`). Every one of
the eleven long functions came along for the ride at its full original length.

Re-measured with a brace matcher over `crates/cyrup-ext-subagents/src/`, string and comment
contents blanked, `#[cfg(test)]` / `#[test]` / `#[tokio::test]` regions excluded:

| lines | location (verified) | function | vs. original task |
|------:|---------------------|----------|-------------------|
| 823 | [`exec/mod.rs:267-1089`](../../crates/cyrup-ext-subagents/src/exec/mod.rs) | `run_sync` | 823 — **unchanged**, moved from `exec/mod.rs:3653` |
| 716 | [`exec/spawn_plan.rs:285-1000`](../../crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) | `build_attempt_spawn_plan_with_read_requirement` | 716 — **unchanged**, moved from `exec/mod.rs:1558` |
| 529 | [`background/runner_main.rs:1201-1729`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | `run_inner` | 529 — **unchanged, same line** |
| 475 | [`exec/drive_attempt.rs:174-648`](../../crates/cyrup-ext-subagents/src/exec/drive_attempt.rs) | `drive_attempt` | 478 → 475, moved from `exec/mod.rs:3093` |
| 458 | [`extension/host/slash.rs:227-684`](../../crates/cyrup-ext-subagents/src/extension/host/slash.rs) | `dispatch_slash` | 458 — **unchanged, same line** |
| 445 | [`extension/executor/foreground.rs:123-567`](../../crates/cyrup-ext-subagents/src/extension/executor/foreground.rs) | `run_foreground_impl` | 445 — **unchanged, same line** |
| 438 | [`extension/tool/routing.rs:241-678`](../../crates/cyrup-ext-subagents/src/extension/tool/routing.rs) | `route_single` | 438 — **unchanged, same line** |
| 411 | [`exec/attempt_runner.rs:95-505`](../../crates/cyrup-ext-subagents/src/exec/attempt_runner.rs) | `run_attempt` | 411 — **unchanged**, moved from `exec/mod.rs:2419` |
| 404 | [`background/runner_main.rs:2375-2778`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | `run_single` | 404 — **unchanged, same line** |
| 403 | [`background/runner_main.rs:513-915`](../../crates/cyrup-ext-subagents/src/background/runner_main.rs) | `run` | 403 — **unchanged, same line** |
| 370 | [`spawn/chain_graph.rs:1417-1786`](../../crates/cyrup-ext-subagents/src/spawn/chain_graph.rs) | `walk_chain` | 370 — unchanged, moved 9 lines up |

Distribution in `cyrup-ext-subagents/src/`, over 2,922 non-test functions: **10 over 400, 11 over
300, 25 over 200, 45 over 150** — bit-for-bit the same >400/>200/>150 counts the original survey
reported. Seven of the eleven are at *literally the same line number* as when the task was filed.
None of the eleven has shrunk meaningfully; none has been split; none has disappeared.

Code/comment split per function (these are not comment-inflated bodies):

```
run_sync                                        823 total   530 code   262 comment    31 blank
build_attempt_spawn_plan_with_read_requirement  716 total   363 code   324 comment    29 blank
run_inner                                       529 total   314 code   192 comment    23 blank
drive_attempt                                   475 total   311 code   157 comment     7 blank
dispatch_slash                                  458 total   239 code   209 comment    10 blank
run_foreground_impl                             445 total   205 code   222 comment    18 blank
route_single                                    438 total   220 code   205 comment    13 blank
run_attempt                                     411 total   251 code   137 comment    23 blank
run_single                                      404 total   172 code   221 comment    11 blank
run (runner_main)                               403 total   261 code   125 comment    17 blank
walk_chain                                      370 total   253 code   105 comment    12 blank
```

`run_sync` still binds **52** `let` locals (`grep -cE '^\s*let '` over lines 267–1089). Its
self-description — "the sole chokepoint every production spawn path in this crate funnels through
(the foreground single-run tool dispatch, the background hop-2 runner's per-step loop, and — via
`chain_graph::walk_chain`/`spawn::parallel::run_bounded`'s `SingleStepExecutor` seam — every chain
step, parallel fan-out child, and dynamic fan-out child as well)" — is verbatim at
[`exec/mod.rs:270-274`](../../crates/cyrup-ext-subagents/src/exec/mod.rs).

**Stale acceptance criterion, now dead:** the original criterion "`src/exec/mod.rs` production line
count drops below 3,500 (from 4,575)" is already satisfied at 1,443 lines — but by the file split,
not by anything this task asks for. It is replaced below by function-level criteria that a file
split cannot accidentally satisfy.

**Scope note:** measured workspace-wide there are 16 functions over 400 lines; six live outside this
crate ([`cyrup-session-svc/src/builder.rs:622-1798`](../../crates/cyrup-session-svc/src/builder.rs)
`build` at 1,177; [`cyrup-ext-sdk/src/example.rs:79-1015`](../../crates/cyrup-ext-sdk/src/example.rs)
`build` at 937; [`cyrup/src/main.rs:100-643`](../../crates/cyrup/src/main.rs) `run` at 544;
[`cyrup-mcp/src/proxy.rs:3059-3597`](../../crates/cyrup-mcp/src/proxy.rs) `execute_call` at 539;
[`cyrup-tui/src/app/events_fold.rs:20-465`](../../crates/cyrup-tui/src/app/events_fold.rs)
`ingest_event_rendered_owned` at 446; and
[`cyrup-modes/src/rpc/mod.rs:855-1262`](../../crates/cyrup-modes/src/rpc/mod.rs) `handle` at 408).
This task stays scoped to `cyrup-ext-subagents` exactly as originally filed; the others are a
separate backlog item, not silently in scope here.

## Why it is expensive

Four of the eleven (`run_sync`, `build_attempt_spawn_plan_with_read_requirement`, `drive_attempt`,
`run_attempt`) form the single-run hot path, 2,425 lines between them. Any change to fallback,
budgets, acceptance or artifacts means establishing ordering constraints by reading 530 lines of
straight-line code and 52 live bindings, and no phase of it can be exercised without spawning a
child process. `background/runner_main.rs` shows the same shape independently: `run` + `run_inner`
+ `run_single` = 1,336 of its 4,587 lines, with `run_inner` being the entire detached-runner turn
loop.

## Sequencing — this is more than one session

Do **not** attempt all eleven in one pass. The prescribed split is four follow-up tasks; land each
as its own commit with `cargo test -p cyrup-ext-subagents` green in between.

1. **`DECOMPOSE_RUN_SYNC`** — `run_sync` alone (§1 below). Highest leverage, highest risk.
2. **`DECOMPOSE_SPAWN_PLAN_AND_ATTEMPT`** — `build_attempt_spawn_plan_with_read_requirement`,
   `run_attempt`, `drive_attempt` (§2–§4).
3. **`DECOMPOSE_BACKGROUND_RUNNER`** — `run`, `run_inner`, `run_single` in `runner_main.rs` (§5–§7).
4. **`DECOMPOSE_EXTENSION_SURFACE`** — `route_single`, `run_foreground_impl`, `dispatch_slash`,
   `walk_chain` (§8–§11).

Split into follow-up task files only if the executor cannot finish in one session; the numbered
sections below are self-contained either way.

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

## §1 — `run_sync` (823 → target ≤ 150)

[`exec/mod.rs:267-1089`](../../crates/cyrup-ext-subagents/src/exec/mod.rs). All line numbers below
verified by reading the file.

### 1a. Collapse the five duplicated early-return `SingleResult` literals — **saves ~175 lines**

Five near-identical 33–43-line `SingleResult { .. }` literals differ only in the `error` string.
Each is marked by the comment `// SUBA-021: no usage budget on this path (see the field doc).` at
lines **284, 317, 364, 431, 475**. The five blocks:

| lines | trigger |
|------:|---------|
| 278–313 | `crate::spawn::depth::is_blocked(&agent.depth)` → `SubagentError::DepthExceeded` |
| 314–346 | `validate_file_only_requires_path(..)` failure |
| 362–404 | `candidates.is_empty()` → `"no candidate model available for this subagent run (empty fallback ladder)"` |
| 430–460 | orchestration skill explicitly requested but missing → `format!("Skills not found: {}", …)` |
| 473–505 | `std::fs::create_dir_all(&scratch_dir)` failure |

Extract one private helper next to `completion_guard_projection` (which already lives at
[`exec/mod.rs:1097`](../../crates/cyrup-ext-subagents/src/exec/mod.rs)):

```rust
/// The `SingleResult` shape every pre-spawn failure in `run_sync` returns: exit 1, no usage,
/// no attempts, no artifacts — only the diagnosis. `usage_budget` is `None` on all of these
/// paths (SUBA-021, see the field doc): nothing was spent because nothing was spawned.
fn pre_spawn_failure(agent: &AgentConfig, task: &str, error: String) -> SingleResult
```

Each of the five sites becomes `return pre_spawn_failure(agent, task, err.to_string());`. Keep the
per-site `// Step 0 (R-SA-055, SAFETY-CRITICAL)` / `// Step 1 (R-SA-025)` / `// T5 (C4)` comments
where they are — they explain the *guard*, not the result shape. Move the SUBA-021 sentence into the
helper's doc comment, as sketched above.

### 1b. Extract the pre-ladder setup phase — **~155 lines**

Lines **405–569** (skill resolution through the structured-output guard) are one coherent phase:
resolve skills → snapshot the output file → make the scratch dir → make the steer inbox/ack/
capability dirs → create the structured-output capture runtime. It produces exactly five values.

```rust
struct LadderSetup {
    skill_injection: String,
    resolved_skill_names: Option<Vec<String>>,
    output_snapshot: OutputSnapshot,          // whatever `snapshot_output_file` returns
    scratch_dir: PathBuf,
    structured_guard: Option<crate::exec::structured::StructuredOutputCleanupGuard>,
}

/// `Err` carries the already-shaped pre-spawn failure result.
async fn prepare_ladder(
    agent: &AgentConfig,
    task: &str,
    opts: &RunOptions,
) -> Result<LadderSetup, Box<SingleResult>>
```

The two inner early returns (430–460, 473–505) become `Err(Box::new(pre_spawn_failure(..)))`; the
call site is
`let setup = match prepare_ladder(agent, task, opts).await { Ok(s) => s, Err(r) => return *r };`.
Note `resolved_skill_names` is already hoisted out of the `else` arm at line **409** precisely
because it outlives the injection string it is computed alongside — carrying both on the struct
makes that explicit instead of load-bearing-by-declaration-order.

The steer-directory block (**511–526**) is three `let _ = std::fs::create_dir_all(..)` calls with
~14 lines of G90/SUBA-049 rationale; keep them inside `prepare_ladder` rather than splitting
further — the rationale is what makes them non-obvious and it should stay adjacent.

### 1c. Extract the ladder-outcome destructure — **~50 lines**

Lines **577–607**: a seven-element tuple destructured out of `match (&last_signal, &last_attempt)`.
Replace the tuple with a named struct and a constructor:

```rust
struct SettledAttempt {
    timed_out: bool,
    interrupted: bool,
    detached: bool,
    process_signal: Option<String>,
    exit_code: i32,
    error: Option<String>,
    final_output: Option<String>,
}

impl SettledAttempt {
    fn from_ladder(signal: Option<&AttemptSignal>, record: Option<&AttemptRecord>) -> Self
}
```

The long comments in that block (the soft-interrupt note on `record.interrupted`, the G104 note on
`process_signal`) move onto the corresponding struct fields as `///` docs. This kills seven
positional bindings whose meaning is currently pure position.

### 1d. Extract output post-processing — **~110 lines**

Lines **609–689** (timeout/turn-budget preamble + output-path handoff) and lines **899–946**
(acceptance-fence strip + truncation + saved-output reference) are the same concern split across the
gates. Extract:

```rust
/// Timeout preamble (pi `execution.ts:824-829`) or, `else if`, the turn-budget terminal note
/// (SUBA-008, `execution.ts:1241-1258`). The `else if` is load-bearing: a timed-out run never
/// also gets a turn-budget preamble.
fn apply_terminal_preamble(
    final_output: Option<String>,
    timed_out: bool,
    timeout_ms: Option<u64>,
    tracker: &crate::exec::turn_budget::TurnBudgetTracker,
) -> Option<String>
```

covering **627–648**, and

```rust
/// R-SA-031 handoff: returns the possibly-replaced output plus the concrete saved path, and
/// folds any handoff error into `error`.
fn resolve_saved_output(
    opts: &RunOptions,
    exit_code: i32,
    final_output: Option<String>,
    output_snapshot: OutputSnapshot,
    error: &mut Option<String>,
) -> (Option<String>, Option<PathBuf>)
```

covering **657–684**. Then a third for the delivery tail:

```rust
/// Strip acceptance fences, apply R-SA-042 truncation, then the saved-output reference message.
/// All three steps are skipped for a detached result (R-SA-037).
fn finalize_delivered_output(
    final_output: Option<String>,
    full_output_for_reference: Option<String>,
    saved_output_path: Option<&PathBuf>,
    detached: bool,
    exit_code: i32,
    max_output: Option<usize>,          // `agent.max_output`
    output_mode: OutputMode,
) -> (Option<String>, bool)             // (delivered, output_truncated)
```

covering **899–946** — the `if !detached` strip at 899, the `(final_output, output_truncated)` match
at 906, and the `match (&saved_output_path, detached)` reference block at 927.

### 1e. Extract the three gates — **~220 lines**

The structured-output check (**705–771**), the completion-mutation guard (**772–822**) and the
acceptance ledger (**824–897**) each mutate the same two variables (`exit_code`, `error`) and each
re-derives a `CleanCompletionGate`. Introduce one mutable carrier and three methods on it:

```rust
struct GateState {
    exit_code: i32,
    error: Option<String>,
    detached: bool,
    interrupted: bool,
    timed_out: bool,
}

impl GateState {
    fn gate(&self) -> CleanCompletionGate { .. }
    /// Step 5 (R-SA-030) — lines 705-771.
    fn apply_structured_output(&mut self, ..) -> Option<serde_json::Value>;
    /// Step 6 (R-SA-034) — lines 772-822; also emits the completion-guard notice on `control`.
    fn apply_completion_guard(&mut self, ..) -> CompletionGuardResult;
    /// Steps 7+8 (R-SA-032/033) — lines 824-897, with R-SA-037's detached bypass.
    async fn apply_acceptance(&mut self, ..) -> Option<AcceptanceLedger>;
}
```

This is the seam the code already announces. The comment at **816–819** explains that the gate must
be *re-derived* after the completion-guard correction ("a run the completion guard already failed
must not additionally run acceptance evaluation against a stale `exit_code == 0` snapshot"). Today
that is enforced by two separately-constructed `CleanCompletionGate` literals (`clean_gate` at 787,
`post_guard_gate` at 819); `GateState::gate()` makes it structural. Each method keeps its full
existing comment block verbatim — those cite upstream `pi` line numbers and are the reason the
ordering is what it is.

The three
`error = Some(match error { Some(existing) if !existing.trim().is_empty() => format!("{existing}; {msg}"), _ => msg })`
blocks (at **752**, **759**, **793**) are the same operation three times; give `GateState` a
`fn push_error(&mut self, message: String)` and call it from all three.

### 1f. Extract the progress snapshot — **~42 lines**

Lines **962–1003** (`if opts.include_progress == Some(true) { .. } else { None }`, Step 10
R-SA-043). Wholly self-contained:

```rust
fn build_progress_snapshot(
    progress: &AgentProgress,
    opts: &RunOptions,
    agent: &AgentConfig,
    task: &str,
    resolved_skill_names: Option<Vec<String>>,
    winning_model: Option<&ModelId>,
    control: &crate::exec::control::ControlMonitor,
    detached: bool,
    interrupted: bool,
    exit_code: i32,
    error: Option<String>,
) -> Option<crate::tui::events::LiveProgressSnapshot>
```

Keep the `status` ladder comment (the "order matters" note at 964–971) inside it.

### 1g. What remains

After 1a–1f, `run_sync`'s body is: depth guard → output-mode validation → `resolve_run_acceptance`
→ `build_model_candidates_scoped` + empty check → `prepare_ladder` → build
`SpawnedChildAttemptRunner` → `run_fallback_ladder` → `SettledAttempt::from_ladder` →
`apply_terminal_preamble` → `resolve_saved_output` → three `GateState` methods →
`finalize_delivered_output` → `build_progress_snapshot` → structured-guard disarm (**1018–1020**) →
usage-budget terminal check (**1032–1043**) → the final `SingleResult` literal (**1045–1088**).

That is roughly 40 statements plus a 44-line result literal — comfortably under 150 lines. The
`SingleResult` literal at the end **stays inline**: it is the function's contract and its field
comments (G77/G104 on `stopped`, the `savedOutputPath` note) belong at the point of publication.

### 1h. Sequencing hint for the C12 acceptance collapse

The known-untracked C12 acceptance collapse also touches `run_sync`: the call at **874** is
`acceptance::evaluate_acceptance_with_cancel`
([`exec/acceptance/lattice/gate.rs:143`](../../crates/cyrup-ext-subagents/src/exec/acceptance/lattice/gate.rs)),
and C12 wants it to end up at
[`exec/acceptance/model/evaluate.rs:50`](../../crates/cyrup-ext-subagents/src/exec/acceptance/model/evaluate.rs)
`model::evaluate_acceptance`. Landing §1 first puts that change inside `GateState::apply_acceptance`
(≈75 lines) instead of inside an 823-line function.

---

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

## Definition of Done

- [ ] No non-test function in `crates/cyrup-ext-subagents/src/` exceeds **300** lines. Verify by
      re-running a brace-matched measurement over that directory with string/comment contents
      blanked and `#[cfg(test)]` / `#[test]` / `#[tokio::test]` regions excluded; the count over 400
      goes from **10 to 0** and the count over 300 from **11 to 0**.
- [ ] `run_sync` ([`exec/mod.rs`](../../crates/cyrup-ext-subagents/src/exec/mod.rs)) is **under 150
      lines** and delegates to named private phase functions.
      `awk 'NR>=<start>&&NR<=<end>' crates/cyrup-ext-subagents/src/exec/mod.rs | grep -cE '^\s*let '`
      over its body drops from **52** to under 20.
- [ ] The five duplicated pre-spawn `SingleResult` literals in `run_sync` are one helper — verify
      with
      `grep -c 'SUBA-021: no usage budget on this path' crates/cyrup-ext-subagents/src/exec/mod.rs`
      returning **1** (from 5).
- [ ] `build_attempt_spawn_plan_with_read_requirement` is under 150 lines and the env-overlay groups
      are separate functions.
- [ ] `walk_chain`'s `DynamicGroup` arm is its own function; `walk_chain` is under 120 lines.
- [ ] `dispatch_slash` is under 60 lines and every non-trivial match arm delegates to a named method.
- [ ] `cargo test -p cyrup-ext-subagents` passes with no new failures. The crate has in-file
      `#[cfg(test)]` modules in all nine touched files (`exec/mod.rs` 2, `exec/spawn_plan.rs` 1,
      `exec/attempt_runner.rs` 1, `background/runner_main.rs` 3, `extension/host/slash.rs` 1,
      `extension/tool/routing.rs` 1, `extension/executor/foreground.rs` 1, `spawn/chain_graph.rs` 1;
      `exec/drive_attempt.rs` has none) — none of them should need editing.
- [ ] `cargo clippy -p cyrup-ext-subagents --all-targets --no-deps -- -D warnings` reports no new
      diagnostics.
- [ ] `cargo test --workspace` passes — `run_sync` is the chokepoint for the whole spawn surface, so
      the crate-scoped suite is not sufficient evidence on its own.
- [ ] `git diff --stat` shows no file outside `crates/cyrup-ext-subagents/src/` touched.

## Source

- Identified by the `subagents-hygiene-survey` workflow (13 agents, 21 raw findings, 16 confirmed
  after adversarial verification).
- Effort: large (four sessions — see **Sequencing** above) · survey priority: 3 of 6
- Re-measured and augmented 2026-08-27; every line number in this document was verified by reading
  the working tree at `df64e81`, whose source tree is level with `origin/main`.
