---
stage: qa
status: completed
updated: 2026-08-27 06:38
---

# Decompose run_sync (823 lines) Into Named Phase Functions

> Split from `DECOMPOSE_LONG_FUNCTIONS` (1 of 7). That task re-measured the crate and found
> **10 non-test functions over 400 lines** — unchanged from the original survey, because the ~30
> merged decomposition PRs split *files*, not *functions*. `exec/mod.rs` went 4,575 → 1,443 lines
> while every long function travelled at full length.

## OBJECTIVE

Reduce `run_sync` from 823 lines to under 150 by extracting eight named private phase functions. This is the highest-leverage and highest-risk item in the set: `run_sync` is the chokepoint for the entire spawn surface, so `cargo test --workspace` is required evidence, not just the crate suite.

| function | location | now | target |
|---|---|---:|---:|
| `run_sync` | [`crates/cyrup-ext-subagents/src/exec/mod.rs:267`](../../crates/cyrup-ext-subagents/src/exec/mod.rs) | 823 | ≤150 |

Total to decompose in this task: **823 lines**.

## Subtasks

### SUBTASK1 — Collapse the five duplicated early-return `SingleResult` literals into one `pre_spawn_failure` helper (~175 lines saved)

**What / where / why:** see §1a in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK2 — Extract the pre-ladder setup phase (~155 lines)

**What / where / why:** see §1b in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK3 — Extract the ladder-outcome destructure (~50 lines)

**What / where / why:** see §1c in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK4 — Extract output post-processing (~110 lines)

**What / where / why:** see §1d in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK5 — Extract the three gates behind a `GateState` carrier (~220 lines) — preserves the post-guard re-derivation order

**What / where / why:** see §1e in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK6 — Extract the progress snapshot (~42 lines)

**What / where / why:** see §1f in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK7 — Verify what remains is under 150 lines and reads as a phase sequence

**What / where / why:** see §1g in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

### SUBTASK8 — Observe the sequencing hint for the C12 acceptance collapse (do not act on it here)

**What / where / why:** see §1h in the research below, which names the concrete seam, the new function signature and the verified line range. The line numbers were re-measured on 2026-08-27 against a tree level with `origin/main`.

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
