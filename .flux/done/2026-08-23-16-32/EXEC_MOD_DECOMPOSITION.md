---
stage: qa
status: completed
updated: 2026-08-23 18:45
---

# Decompose exec/mod.rs Into Submodules

## Description

`crates/cyrup-ext-subagents/src/exec/mod.rs` is 7,923 lines — by far the largest file in the
crate (next-largest sibling is `mcp_direct_tools.rs` at 2,816; the 19 submodules `exec/mod.rs`
already declares run 300–2,816 lines each, tests included). This crate is a Rust port of an
upstream TypeScript codebase ("pi-subagents"); nearly every item here carries a doc comment tying
it to a `R-SA-###`/`arch-SA §#.#`/`func-SA §#.#` requirement ID and an upstream `pi` source
location. **Preserve every doc comment verbatim when moving code** — they are the crate's
requirement-traceability record, not incidental commentary.

**DO THE SPLIT.** This is an implementation task: physically move the code below into the new
files, fix visibility, keep the crate compiling, and land it. Do not stop at another document —
the plan has already been researched (below); executing it is the deliverable.

### The file's own section banners already mark 5 top-level concerns

`exec/mod.rs` contains five `// ====...====` banner comments the original author left as section
dividers (grep `^// ====`). They line up exactly with the natural seams found by reading the file,
and are strong evidence of author intent:

| Banner (verbatim)                                                                          | Line |
|-----------------------------------------------------------------------------------------------|------|
| `AgentConfig / RunOptions / SingleResult (arch-SA §3.4)`                                      | 225  |
| `AgentProgress: the live per-attempt fold (R-SA-027/028)`                                     | 1001 |
| `SubagentSpawner: the seam production spawning goes through (mirrors AttemptRunner's own …)`  | 1301 |
| `run_sync: the model-fallback attempt loop, wired end to end (arch-SA §6.3.2)`                | 3569 |
| `plan_batch: eager whole-batch fork-context resolution (arch-SA §6.6, R-SA-137)`              | 4521 |

The third banner ("SubagentSpawner") spans **2,268 lines** (1301–3568) and is itself three
distinct concerns bundled together (spawn-plan *construction*, the concrete `AttemptRunner`
*process-spawning* impl, and the per-attempt NDJSON *drive loop*) — split it into three files, not
one.

`#[cfg(test)] mod tests { use super::*; ... }` occupies the trailing 3,351 lines (4573–7923) as
one flat block with no internal `mod` nesting — every test in it moves to the new file that owns
the function/type it exercises (see "Test triage" below). Line numbers throughout this task are
against the file as it stands before any extraction begins; re-derive them as the file shrinks
(grep for the item name rather than trusting a stale absolute number after step 1).

## Full item inventory (absolute line numbers, current file)

```
1     module doc comment (`//!`)
39    19× `pub mod <name>;` submodule declarations, each with its own doc comment
127   imports (`use` block)
160   RECENT_OUTPUT_CAP / RECENT_OUTPUT_TAIL_LINES / RECENT_OUTPUT_LINE_CHARS consts
202   fn is_false(&bool) -> bool                              [pub(crate)]
206   fn bound_output_line(&str) -> String                    [private]
221   fn format_timeout_message(u64) -> String                [pub]
--- banner: AgentConfig / RunOptions / SingleResult (225) ---
234   struct AgentConfig { .. }                                [pub]  + impl (303)
358   struct ResolvedAgentPersona { .. }                       [pub]  + impl (429)
500   fn resolve_step_agent_config(&AgentDefinition) -> ..     [pub]
515   struct RunOptions { .. }  (~240 lines, 35 pub fields)     [pub]
756   struct LiveEventSink { .. }                              [pub]  + Debug (770) + impl (776)
843   struct SingleResult { .. }  (~167 lines)                 [pub]
--- banner: AgentProgress (1001) ---
1011  struct AgentProgress { .. }                              [pub]  + impl (1069)
1272  struct ProgressSnapshotInput<'a> { .. }                  [pub]
--- banner: SubagentSpawner (1301) — split into 3 files, see below ---
1316  struct AttemptSpawnPlan { .. }                           [pub]
1397  fn apply_thinking_suffix(..) -> Option<String>            [pub]
1424  fn split_known_thinking_suffix(&str) -> (&str, &str)      [pub]
1437  fn push_unique(&mut Vec<String>, String)                  [private]
1524  fn build_attempt_spawn_plan(..) -> ..                     [pub]
1558  fn build_attempt_spawn_plan_with_read_requirement(..)     [pub]  (766 lines — largest fn in file)
2325  fn build_task_text(..) -> String                          [private -> needs pub(crate)]
2359  struct SpawnedChildAttemptRunner<'a> { .. }                [private -> needs pub(crate)]
2390  struct AttemptRecord { .. }                                [private -> needs pub(crate) + fields]
2416  impl AttemptRunner for SpawnedChildAttemptRunner<'_> { .. } (477 lines)
2761       calls structured_output_absent(..) — SEE NOTE BELOW, this call is INSIDE this impl block
2893  fn process_signal_name(&ExitStatus) -> Option<String>      [private, cfg(unix)/cfg(not(unix)) pair]
2907  struct DriveOutcome { .. }                                 [private -> needs pub(crate) + fields]
2972  fn structured_output_absent(..) -> ..                      [private — SEE NOTE BELOW re: placement]
3011  fn contact_supervisor_block_prompt(&SubagentEvent) -> ..   [private]
3031  fn is_assistant_message_end(&SubagentEvent) -> bool         [private]
3040  fn message_end_tool_calls(&SubagentEvent) -> Vec<..>        [private]
3059  fn message_end_has_tool_call(&SubagentEvent) -> bool        [private]
3071  fn is_sole_structured_output_tool_call(&SubagentEvent) -> bool [private]
3093  async fn drive_attempt(..) -> DriveOutcome                  [private -> needs pub(crate)]  (491 lines)
--- banner: run_sync (3569) ---
3585  fn resolve_run_acceptance(..) -> AcceptanceContract         [private]
3650  pub async fn run_sync(&AgentConfig, &str, &RunOptions) -> SingleResult   (829 lines — biggest fn in file)
4480  fn completion_guard_projection(&AgentConfig) -> AgentDefinition [private]
--- banner: plan_batch (4521) ---
4528  struct BatchForkRequest { .. }                             [pub]
4560  pub async fn plan_batch(&ForkContextResolver, &[BatchForkRequest]) -> ..
4572  #[cfg(test)] mod tests { .. }   (3,351 lines, flat, `use super::*;`)
```

## Cross-file call graph (traced by grep against the real file — verify again before moving code,
the split itself will shift line numbers as you go)

This is the load-bearing part of the plan — it determines exactly which items need a visibility
bump and to what:

- **`build_task_text`** is called from inside `impl AttemptRunner for SpawnedChildAttemptRunner`
  (line 2468) — i.e. from the *attempt-runner* concern, not from `run_sync`. It stays defined
  alongside the other spawn-plan builders but must go from private `fn` to `pub(crate) fn` once
  its caller moves to a different file.
- **`SpawnedChildAttemptRunner { .. }`** is constructed exactly once, at line 3938, **inside
  `run_sync`** — a struct-literal construction, which means every field `run_sync` sets must be
  visible from `mod.rs`, not just the struct itself. `SpawnedChildAttemptRunner` and its
  currently-private fields need `pub(crate)`.
- **`process_signal_name`** is called only at line 2605, inside the same `AttemptRunner` impl that
  defines it (line 2416) — it can stay **private** to whichever file houses that impl; no
  visibility bump needed as long as it moves together with the impl.
- **`drive_attempt`** is called only at line 2575, inside the `AttemptRunner` impl — but per the
  file split below, `drive_attempt` moves to its own file (the drive-loop concern is distinct from
  the process-spawning concern even though today they're adjacent). That makes this a cross-file
  call: `async fn drive_attempt` needs `pub(crate)`.
- **`DriveOutcome`**'s fields (`interrupted`, `detached`, `turn_budget`, …) are read by direct
  field access at the `drive_attempt(...).await` call site (lines 2582–2599 confirm
  `outcome.interrupted`, `outcome.detached`, `outcome.turn_budget`) — from the *attempt-runner*
  file, once `drive_attempt` moves out of it. `DriveOutcome` and its fields need `pub(crate)`.
- **`AttemptRecord`**'s fields (`progress`, `final_output`, `interrupted`, `control`,
  `turn_budget`) are populated by the `AttemptRunner` impl and consumed by `run_fallback_ladder`
  (in the already-separate `fallback.rs`) and ultimately by `run_sync`. `AttemptRecord` and its
  fields need `pub(crate)`.
- **`resolve_run_acceptance`** is called only from inside `run_sync` (line 3730) plus its own
  tests. It has no other caller — **leave it where `run_sync` ends up** (mod.rs root); do not
  extract it separately.
- **`build_attempt_spawn_plan` / `build_attempt_spawn_plan_with_read_requirement`** are already
  `pub fn` (crate-visible today only because everything sits in one file) — no visibility change
  needed, just an import at each new call site (the `AttemptRunner` impl at line 2486, and dozens
  of tests).
- ⚠️ **CORRECTED BY QA — `structured_output_absent` is NOT part of the drive-loop group.** Its
  ONLY call site in the whole file is line 2761, **inside `impl AttemptRunner for
  SpawnedChildAttemptRunner`** (i.e. inside `attempt_runner.rs`'s territory, NOT inside
  `drive_attempt`'s body — `drive_attempt` itself never calls it). An earlier draft of this plan
  grouped `structured_output_absent` with the other five NDJSON-event helpers below and claimed
  "no call sites outside 2972–3584"; that was checked and is **wrong** for this one function
  specifically (2761 < 2972). **Fix**: move `structured_output_absent` into `attempt_runner.rs`
  (co-located with its actual and only caller), staying **private** there — do NOT put it in
  `drive_attempt.rs` and do NOT give it `pub(crate)`; neither is needed once it's in the right
  file.
- The other five NDJSON-event classification helpers (`contact_supervisor_block_prompt`,
  `is_assistant_message_end`, `message_end_tool_calls`, `message_end_has_tool_call`,
  `is_sole_structured_output_tool_call`) genuinely are used **only inside `drive_attempt`'s own
  body** (verified: their sole call sites are lines 3292/3321/3060,3072/3322/3326, all within
  2972–3584). These five stay **private**, moving as a unit with `drive_attempt` into
  `drive_attempt.rs`.

## Target layout — create these files

All new files live in `src/exec/`, alongside the 19 already-declared submodules, using the same
naming register (short noun phrases, snake_case) those already use. Declare each as `pub mod
<name>;` in `mod.rs` — matching how the 19 existing submodules are declared — not `mod` (private),
since sibling submodules already reach into each other via `crate::exec::<name>::<item>` and the
new files need the same reachability.

1. **`exec/agent_config.rs`** (~570 lines) — `AgentConfig` + its `impl`, `ResolvedAgentPersona` +
   its `impl`, `resolve_step_agent_config`, `RunOptions`, `LiveEventSink` + `Debug` + `impl`.
   *Rationale*: the static, serializable "what to run and how" input surface — everything a
   caller assembles once, before any process exists. No process/IO logic lives here. Also
   re-export or move `is_false` here if `SingleResult`'s `#[serde(skip_serializing_if =
   "crate::exec::is_false")]` fields (lines 883/912/918) end up in this file's sibling
   `run_result.rs` instead — check where `is_false` is actually referenced (`grep -n
   "is_false" src/exec/mod.rs`) before deciding whether it stays at the root or moves; either is
   fine as long as the `skip_serializing_if` path string is updated to match.
2. **`exec/run_result.rs`** (~170 lines) — `SingleResult`.
   *Rationale*: the output contract, symmetric with `agent_config.rs`'s input contract. Small
   enough it could stay merged into `agent_config.rs` (the original banner groups all three
   together) — split it out only if `agent_config.rs` otherwise crosses ~700 lines once its tests
   land; otherwise keep `SingleResult` in `agent_config.rs` and skip this file. Either is
   defensible; do not create a third variant that scatters these types across more than two files.
3. **`exec/progress.rs`** (~305 lines) — `AgentProgress` + `impl`, `ProgressSnapshotInput`.
   *Rationale*: exactly the banner's own "the live per-attempt fold" scope; already
   self-contained (no cross-references found outside its own banner range other than
   construction by `drive_attempt`/`run_sync`, which is fine — normal producer/consumer coupling
   between root and leaf modules).
4. **`exec/spawn_plan.rs`** (~1,043 lines) — `AttemptSpawnPlan`, `apply_thinking_suffix`,
   `split_known_thinking_suffix`, `push_unique`, `build_attempt_spawn_plan`,
   `build_attempt_spawn_plan_with_read_requirement`, `build_task_text` (bump to `pub(crate)`).
   *Rationale*: pure computation of *what to spawn* — argv/env/prompt/system-prompt assembly, zero
   process handles, zero I/O. This is the largest new leaf file, dominated by one 766-line
   function; that function is a single cohesive builder (confirmed: it is not internally callable
   in parts by anything else) and should not be force-split further without deeper justification
   than file-size alone.
5. **`exec/attempt_runner.rs`** (~650 lines) — `SpawnedChildAttemptRunner` (bump to
   `pub(crate)`), `AttemptRecord` (bump to `pub(crate)`, fields too), `impl AttemptRunner for
   SpawnedChildAttemptRunner<'_>`, `process_signal_name` (stays private, moves with its only
   caller), **`structured_output_absent`** (stays private, moves here per the QA correction above
   — NOT into `drive_attempt.rs`).
   *Rationale*: the concrete production implementor of the `AttemptRunner` trait (trait itself
   already lives in `fallback.rs`) — the seam that spawns one real OS child process per
   model-fallback attempt via `crate::spawn::SpawnedChild::spawn`. Mirrors the "production seam"
   framing the original banner comment already used.
6. **`exec/drive_attempt.rs`** (~570 lines) — `DriveOutcome` (bump to `pub(crate)`, fields too),
   `contact_supervisor_block_prompt`, `is_assistant_message_end`, `message_end_tool_calls`,
   `message_end_has_tool_call`, `is_sole_structured_output_tool_call`, `drive_attempt` (bump to
   `pub(crate)`). Do **not** include `structured_output_absent` here (see QA correction above).
   *Rationale*: "given one already-spawned child's stdout event stream, fold it into
   progress/output/acceptance state until it settles" is a distinct concern from *how the child
   was spawned* (`attempt_runner.rs`) or *what argv it was spawned with* (`spawn_plan.rs`) —
   swapping the drive loop's NDJSON-folding logic should never require touching process-spawn
   code and vice versa.
7. **`exec/mod.rs` (slimmed root, ~950 lines before tests)** — keeps: module doc comment, the
   `pub mod` declarations (19 existing + the 5–6 new ones added here), the `RECENT_OUTPUT_*`
   consts, `bound_output_line`/`format_timeout_message` (and `is_false` unless relocated per item
   1 above), `resolve_run_acceptance`, `run_sync` (829 lines), `completion_guard_projection`,
   `BatchForkRequest`, `plan_batch`.
   *Rationale*: the module doc comment (lines 1–35) **explicitly self-describes** mod.rs as "the
   integration module: it owns `run_sync`/`RunOptions`/`AgentConfig`/`SingleResult`… wiring
   together every sibling module in this subtree" — keep that claim true. `run_sync` is the one
   function that legitimately touches every other concern (config, progress, spawn-plan,
   attempt-runner, acceptance, completion-guard, usage/turn budgets) and reads best as the root's
   own orchestration, not as one more leaf. `resolve_run_acceptance` and
   `completion_guard_projection` have no callers outside `run_sync` and move with it by
   construction (no reason to relocate a private helper away from its sole caller).
   `plan_batch`/`BatchForkRequest` are their own banner-marked concern but are tiny (43 lines
   total) — leave at the root rather than creating a one-function file.

This yields 6 new files (or 7, if `run_result.rs` is split out per item 2) plus a root that drops
from 7,923 to roughly 950 lines of production code — in line with the existing sibling range
(300–2,816).

## Visibility summary (everything that must change)

| Item | Current | Required |
|---|---|---|
| `build_task_text` | private `fn` | `pub(crate) fn` |
| `SpawnedChildAttemptRunner` (struct + fields) | private | `pub(crate)` |
| `AttemptRecord` (struct + fields) | private | `pub(crate)` |
| `DriveOutcome` (struct + fields) | private | `pub(crate)` |
| `drive_attempt` | private `async fn` | `pub(crate) async fn` |
| `process_signal_name` | private | **no change** (moves with sole caller) |
| `structured_output_absent` | private | **no change** — moves to `attempt_runner.rs` with its sole caller (QA correction) |
| the other 5 NDJSON-event helpers under `drive_attempt` | private | **no change** (move as a unit into `drive_attempt.rs`) |
| `resolve_run_acceptance`, `completion_guard_projection` | private | **no change** (stay at root) |
| everything already `pub` (`AgentConfig`, `RunOptions`, `SingleResult`, `AgentProgress`,
  `AttemptSpawnPlan`, `build_attempt_spawn_plan[_with_read_requirement]`, `BatchForkRequest`,
  `plan_batch`, `run_sync`, `format_timeout_message`, …) | `pub` | **no change**, just re-export
  or `use crate::exec::<new_module>::*` at each new call site |

## Test triage (4,573–7,923, 3,351 lines, one flat `mod tests { use super::*; }`)

No internal `mod` nesting exists today, so every `#[test]` fn moves by inspecting which production
item(s) it calls. The call-graph trace above already establishes the biggest bucket: **the
majority of these tests call `build_attempt_spawn_plan`/`build_attempt_spawn_plan_with_read_requirement`
directly** (50+ distinct call sites across the test block) — these move to `spawn_plan.rs`'s own
`#[cfg(test)] mod tests`. Route the remainder the same way:

- Tests calling `structured_output_absent` directly (lines 6365–6381) → `attempt_runner.rs` (QA
  correction — NOT `drive_attempt.rs`).
- Tests calling `process_signal_name` directly (e.g. the POSIX-signal-name test at line 4592) →
  `attempt_runner.rs`.
- Tests calling `resolve_run_acceptance` directly (lines 7711–7731) → stay in `mod.rs`'s own test
  module (co-located with the function, per the visibility table above).
- Tests calling `run_sync` end-to-end (spanning config + spawn + drive + acceptance in one
  assertion) → stay in `mod.rs`'s test module; these are integration-style by nature and do not
  belong to any single leaf file.
- Tests calling `build_task_text` directly but not `build_attempt_spawn_plan` → `spawn_plan.rs`.
- Any remaining test exercising `AgentProgress`/`ProgressSnapshotInput` construction/methods in
  isolation → `progress.rs`.
- Any remaining test constructing `AgentConfig`/`RunOptions`/`ResolvedAgentPersona` in isolation
  (no spawn/drive/run_sync call) → `agent_config.rs`.

Each new file's test module keeps the crate-standard shape already used by every sibling
(`#[cfg(test)] mod tests { use super::*; ... }`, with the same `#![allow(clippy::unwrap_used,
clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` the current block already carries
at line 4576 — copy that allow-list onto every new test module that needs it, don't invent a
narrower one per file).

## Execution order (DO THESE STEPS — this is the actual task)

Extract leaves before roots, so at every intermediate step `cargo check -p cyrup-ext-subagents`
passes. Re-grep line numbers before each step — the file shrinks as you go and stale line numbers
will point at the wrong code.

1. Create `exec/progress.rs` — move `AgentProgress`, its `impl`, `ProgressSnapshotInput`, and
   their tests out of `mod.rs`. Declare `pub mod progress;` in `mod.rs`. `cargo check`.
2. Create `exec/agent_config.rs` (+ `exec/run_result.rs` if splitting `SingleResult` out per item
   2 above) — move `AgentConfig`, `ResolvedAgentPersona`, `resolve_step_agent_config`,
   `RunOptions`, `LiveEventSink`, `SingleResult`, and their tests. Resolve the `is_false` location
   per item 1's note. Declare the new `pub mod`(s). `cargo check`.
3. Create `exec/spawn_plan.rs` — move `AttemptSpawnPlan`, `apply_thinking_suffix`,
   `split_known_thinking_suffix`, `push_unique`, `build_attempt_spawn_plan`,
   `build_attempt_spawn_plan_with_read_requirement`, `build_task_text` (bump to `pub(crate)`), and
   the 50+ tests that call them. Declare `pub mod spawn_plan;`. `cargo check`.
4. Create `exec/drive_attempt.rs` — move `DriveOutcome` (bump `pub(crate)`, fields too),
   `contact_supervisor_block_prompt`, `is_assistant_message_end`, `message_end_tool_calls`,
   `message_end_has_tool_call`, `is_sole_structured_output_tool_call`, `drive_attempt` (bump
   `pub(crate)`), and their tests. Declare `pub mod drive_attempt;`. `cargo check`.
5. Create `exec/attempt_runner.rs` — move `SpawnedChildAttemptRunner` (bump `pub(crate)`),
   `AttemptRecord` (bump `pub(crate)`, fields too), `impl AttemptRunner for
   SpawnedChildAttemptRunner<'_>`, `process_signal_name`, `structured_output_absent` (per QA
   correction — this file, not step 4), and their tests. Import `build_attempt_spawn_plan_with_read_requirement`/
   `build_task_text` from `spawn_plan` and `drive_attempt` from `drive_attempt`. Declare `pub mod
   attempt_runner;`. `cargo check`.
6. In `mod.rs`: update the `use` block to pull in `SpawnedChildAttemptRunner`/`AttemptRecord` from
   `crate::exec::attempt_runner` (used at the single `run_sync` construction site, line ~3938 in
   the original file). Confirm `run_sync` does not call `drive_attempt`/`DriveOutcome`/
   `build_task_text`/`structured_output_absent` directly — it only reaches them through
   `SpawnedChildAttemptRunner` — so no further imports should be needed there.
7. Move the remaining tests (those exercising `run_sync`/`resolve_run_acceptance`/`plan_batch`
   end-to-end) to stay in `mod.rs`'s own `#[cfg(test)] mod tests`.
8. Grep every moved doc comment for `[`crate::exec::...`]` intra-doc links (the file uses them
   extensively, e.g. `[`drive_attempt`]`, `[`DriveOutcome`]`) and fix the path prefix wherever the
   referenced item's module changed — `rustdoc` link resolution will otherwise silently point at a
   dead path after the split. Preserve every doc comment's text verbatim; only the module path in
   intra-doc links may change.
9. Run `cargo check -p cyrup-ext-subagents` and `cargo test -p cyrup-ext-subagents` on the final
   state. Both must be clean. Run `cargo clippy -p cyrup-ext-subagents --all-targets` too, since
   the moved test modules carry clippy allow-lists that must still apply correctly once relocated.

## Definition of done

- [ ] `exec/mod.rs` reduced from 7,923 lines to roughly 950 lines of production code (module doc,
      `pub mod` declarations, consts, `run_sync`, `resolve_run_acceptance`,
      `completion_guard_projection`, `plan_batch`, `BatchForkRequest`, plus its own trimmed test
      module) — verify with `wc -l`.
- [ ] `exec/progress.rs`, `exec/agent_config.rs` (+ optional `exec/run_result.rs`),
      `exec/spawn_plan.rs`, `exec/attempt_runner.rs`, `exec/drive_attempt.rs` all exist, each
      declared `pub mod` in `mod.rs`, each containing the items listed above plus their triaged
      tests.
- [ ] `structured_output_absent` lives in `exec/attempt_runner.rs` (NOT `drive_attempt.rs`),
      staying private, per the QA correction.
- [ ] Every doc comment moved verbatim; every intra-doc `[`crate::exec::...`]` link updated to its
      item's new module path.
- [ ] `cargo check -p cyrup-ext-subagents` is clean.
- [ ] `cargo test -p cyrup-ext-subagents` passes (no test dropped, none newly ignored).
- [ ] `cargo clippy -p cyrup-ext-subagents --all-targets` is clean.
