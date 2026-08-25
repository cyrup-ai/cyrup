---
stage: qa
status: completed
updated: 2026-08-24 14:45
---

# Broken Intra Doc Links

## Objective

The `exec/mod.rs` decomposition (commit range `ebf328c8..HEAD`, PR branch
`claude/largest-rust-file-ynudqz`) split `agent_config.rs`, `run_result.rs`,
`attempt_runner.rs`, `drive_attempt.rs`, `progress.rs`, and `spawn_plan.rs` out of
`crates/cyrup-ext-subagents/src/exec/mod.rs`, carrying their doc comments over **verbatim**. Those
doc comments use bare, unqualified `[`ident`]` intra-doc links that only resolved because the text
used to live directly inside `exec/mod.rs`'s own module scope (child `mod`s and sibling items are
directly nameable, unqualified, from a module's own body). Now that the code lives in separate
sibling modules, Rust's module system does not inherit that scope, so every one of these bare paths
fails to resolve.

The workspace pins `[workspace.lints.rustdoc] broken_intra_doc_links = "deny"` in the root
`Cargo.toml` (labeled "rustdoc lints (regrowth gate)"), inherited by every crate via
`[lints] workspace = true`. This is a **hard `cargo doc` build failure**, not a lint warning:

```
$ cargo doc -p cyrup-ext-subagents --no-deps --lib
error: unresolved link to `completion_guard::expects_implementation_mutation`
  --> crates/cyrup-ext-subagents/src/exec/agent_config.rs:33:56
   | no item named `completion_guard` in scope
...
error: could not document `cyrup-ext-subagents`
```

Verified against the merge-base: `cargo doc -p cyrup-ext-subagents --no-deps --lib` gives **0**
unresolved-link errors at `ebf328c87830a5ebf84a2c48f640e5aa7602cee9` and **58** on the current
branch head, all inside files this same PR touches — nothing pre-existing, nothing out of scope.

## Definition of Done

`cargo doc -p cyrup-ext-subagents --no-deps --lib` exits 0 with zero `unresolved link` errors.
(The workspace already builds docs with `--document-private-items`, via
`.cargo/config.toml`'s `rustdocflags`, so links into `pub(crate)`/private items are expected to
resolve too — do not `pub`-ify anything just to make a link work.)

## The three resolution rules

Every one of the 58 sites below is fixed by requalifying the bare path. Which qualified form to use
depends on where the target actually lives — apply whichever of these three rules matches:

1. **Item lives in a module re-exported by glob** (`agent_config`, `progress`, `run_result`,
   `spawn_plan` are all `pub use ...::*;`'d back into `crate::exec` at `exec/mod.rs:94,96-98`, and
   `RECENT_OUTPUT_CAP`/`RECENT_OUTPUT_TAIL_LINES`/`RECENT_OUTPUT_LINE_CHARS`/`run_sync`/`plan_batch`
   are declared directly in `exec/mod.rs`, i.e. already *are* `crate::exec`) →
   use the **flattened** path: `crate::exec::<Item>` (not `crate::exec::agent_config::<Item>`).
   This matches the style the PR already uses elsewhere (e.g. `crate::exec::acceptance::...`).

2. **Item lives in a module declared with plain `pub mod X;`** (`completion_guard`, `acceptance`,
   `output`, `fallback`, `tool_call_summary`, `ndjson`, `attempt_runner`, `drive_attempt`,
   `structured`, `control`, `turn_budget` — no accompanying glob re-export) →
   use the **module-qualified** path: `crate::exec::<module>::<Item>`.

3. **Item lives in a different crate or a different top-level module of this crate**
   (`cyrup_core::ModelId`, `crate::discovery::types::AgentDefinition`) →
   use that item's **real** path. One link in this batch (`formatters.rs:26`) is not just
   under-qualified but pointing at the wrong place entirely — `crate::background::ModelId` — because
   `ModelId` was never defined in `background`; it is `cyrup_core::ModelId` (see
   `crates/cyrup-ext-subagents/src/exec/agent_config.rs:9`,
   `crates/cyrup-ext-subagents/src/background/mod.rs:67` for existing correct imports). This one is
   a genuine wrong-target fix, not a re-qualification.

### The one non-mechanical case: trait-impl methods

`SpawnedChildAttemptRunner::run_attempt` (`exec/mod.rs:261`, `exec/drive_attempt.rs:19`) is a
method from `impl AttemptRunner for SpawnedChildAttemptRunner<'_>`
(`exec/attempt_runner.rs:92-96`) — **not** an inherent method. Confirmed experimentally (in a
disposable worktree, reverted, not part of this diff):

- `` [`crate::exec::attempt_runner::SpawnedChildAttemptRunner::run_attempt`] `` fails: *"the struct
  `SpawnedChildAttemptRunner` has no field or associated item named `run_attempt`"* — rustdoc's
  `Type::method` path resolution only walks inherent impls, not trait impls, for a local struct.
- `` [`<crate::exec::attempt_runner::SpawnedChildAttemptRunner as crate::exec::fallback::AttemptRunner>::run_attempt`] ``
  fails: *"fully-qualified syntax is unsupported"* in intra-doc links.
- `` [`crate::exec::fallback::AttemptRunner::run_attempt`] `` (link to the **trait's** method
  definition instead of the impl) **resolves**. Use this form at both sites.

## Fix table

All 58 sites, by file. Each row's "Fix" is the exact replacement for the bracketed link text on
that line (surrounding prose is unchanged). Apply literally — do not re-derive targets, they are
confirmed above and via the grep/read commands used to build this table.

### `exec/agent_config.rs` (22 sites)

| Line | Broken link | Fix |
|---|---|---|
| 33 | `` [`completion_guard::expects_implementation_mutation`] `` | `` [`crate::exec::completion_guard::expects_implementation_mutation`] `` |
| 34 | `` [`acceptance::AcceptanceContract::heuristic_default`] `` | `` [`crate::exec::acceptance::AcceptanceContract::heuristic_default`] `` |
| 40 | `` [`apply_thinking_suffix`] `` | `` [`crate::exec::apply_thinking_suffix`] `` |
| 77 | `` [`output::OutputCap`] `` | `` [`crate::exec::output::OutputCap`] `` |
| 304 | `` [`fallback`] `` | `` [`crate::exec::fallback`] `` |
| 308 | `` [`run_sync`] `` | `` [`crate::exec::run_sync`] `` |
| 323 | `` [`run_sync`] `` | `` [`crate::exec::run_sync`] `` |
| 335 | `` [`build_task_text`] `` | `` [`crate::exec::build_task_text`] `` |
| 346 | `` [`run_sync`] `` | `` [`crate::exec::run_sync`] `` |
| 356 | `` [`SingleResult`] `` | `` [`crate::exec::SingleResult`] `` |
| 363 | `` [`build_attempt_spawn_plan`] `` | `` [`crate::exec::build_attempt_spawn_plan`] `` |
| 386 | `` [`run_sync`] `` | `` [`crate::exec::run_sync`] `` |
| 387 | `` [`SingleResult::progress`] `` | `` [`crate::exec::SingleResult::progress`] `` |
| 392 | `` [`SingleResult`] `` | `` [`crate::exec::SingleResult`] `` |
| 408 | `` [`plan_batch`] `` | `` [`crate::exec::plan_batch`] `` |
| 419 | `` [`PARENT_SESSION_ENV_VAR`] `` | `` [`crate::exec::PARENT_SESSION_ENV_VAR`] `` |
| 427 | `` [`drive_attempt`] `` | `` [`crate::exec::drive_attempt::drive_attempt`] `` |
| 521 | `` [`SingleResult::control_events`] `` | `` [`crate::exec::SingleResult::control_events`] `` |
| 548 | `` [`run_sync`] `` | `` [`crate::exec::run_sync`] `` |
| 621 | `` [`fallback::ModelAttempt`] `` | `` [`crate::exec::fallback::ModelAttempt`] `` |
| 625 | `` [`SingleResult::tool_calls`] `` | `` [`crate::exec::SingleResult::tool_calls`] `` |
| 626 | `` [`tool_call_summary::ToolCallSummary`] `` | `` [`crate::exec::tool_call_summary::ToolCallSummary`] `` |

(Line 548 also has `[`RunOptions::live_events`]` on the same line — leave it as-is, `RunOptions` is
defined in this same file and already resolves.)

### `exec/run_result.rs` (7 sites)

| Line | Broken link | Fix |
|---|---|---|
| 15 | `` [`RunOptions::include_progress`] `` | `` [`crate::exec::RunOptions::include_progress`] `` |
| 39 | `` [`drive_attempt`] `` | `` [`crate::exec::drive_attempt::drive_attempt`] `` |
| 41 | `` [`RunOptions::clarify`] `` | `` [`crate::exec::RunOptions::clarify`] `` |
| 136 | `` [`output::truncate_output`] `` | `` [`crate::exec::output::truncate_output`] `` |
| 149 | `` [`RunOptions::include_progress`] `` | `` [`crate::exec::RunOptions::include_progress`] `` |
| 168 | `` [`RECENT_OUTPUT_CAP`] `` | `` [`crate::exec::RECENT_OUTPUT_CAP`] `` |
| 168 | `` [`RECENT_OUTPUT_LINE_CHARS`] `` | `` [`crate::exec::RECENT_OUTPUT_LINE_CHARS`] `` |

(Line 15 also has `[`Self::progress`]` — leave as-is, resolves locally.)

### `exec/attempt_runner.rs` (8 sites)

| Line | Broken link | Fix |
|---|---|---|
| 32 | `` [`ndjson::consume_stdout`] `` | `` [`crate::exec::ndjson::consume_stdout`] `` |
| 43 | `` [`run_sync`] `` | `` [`crate::exec::run_sync`] `` |
| 49 | `` [`run_sync`] `` | `` [`crate::exec::run_sync`] `` |
| 72 | `` [`SingleResult`] `` | `` [`crate::exec::SingleResult`] `` |
| 80 | `` [`SingleResult::control_events`] `` | `` [`crate::exec::SingleResult::control_events`] `` |
| 84 | `` [`SingleResult`] `` | `` [`crate::exec::SingleResult`] `` |
| 588 | `` [`run_sync`] `` | `` [`crate::exec::run_sync`] `` |
| 601 | `` [`run_sync`] `` | `` [`crate::exec::run_sync`] `` |

(Lines 30-31, 74 etc. also reference `[`AttemptRunner`]`, `[`SpawnedChild::spawn`]`,
`[`AttemptRecord`]`, `[`AttemptSignal`]`, `[`Self::skill_injection`]` — none of those are in the
broken list; leave them exactly as they are.)

### `exec/drive_attempt.rs` (1 site)

| Line | Broken link | Fix |
|---|---|---|
| 19 | `` [`SpawnedChildAttemptRunner::run_attempt`] `` | `` [`crate::exec::fallback::AttemptRunner::run_attempt`] `` (trait-method case — see above, not a plain requalification) |

### `exec/mod.rs` (1 site)

| Line | Broken link | Fix |
|---|---|---|
| 261 | `` [`crate::exec::attempt_runner::SpawnedChildAttemptRunner::run_attempt`] `` | `` [`crate::exec::fallback::AttemptRunner::run_attempt`] `` (same trait-method case; this one is already fully qualified but pointing at the wrong kind of path) |

### `exec/progress.rs` (13 lines, 14 sites)

| Line | Broken link | Fix |
|---|---|---|
| 21 | `` [`SingleResult`] `` | `` [`crate::exec::SingleResult`] `` |
| 28 | `` [`fallback::run_fallback_ladder`] `` | `` [`crate::exec::fallback::run_fallback_ladder`] `` |
| 46 | `` [`SingleResult::progress`] `` | `` [`crate::exec::SingleResult::progress`] `` |
| 48 | `` [`RECENT_OUTPUT_LINE_CHARS`] `` | `` [`crate::exec::RECENT_OUTPUT_LINE_CHARS`] `` |
| 52 | `` [`output::extract_final_output`] `` | `` [`crate::exec::output::extract_final_output`] `` |
| 53 | `` [`completion_guard::has_mutation_tool_call`] `` | `` [`crate::exec::completion_guard::has_mutation_tool_call`] `` |
| 53 | `` [`evaluate_completion_mutation_guard`] `` | `` [`crate::exec::completion_guard::evaluate_completion_mutation_guard`] `` |
| 57 | `` [`completion_guard::has_mutation_tool_call`] `` | `` [`crate::exec::completion_guard::has_mutation_tool_call`] `` |
| 58 | `` [`SingleResult`] `` | `` [`crate::exec::SingleResult`] `` |
| 86 | `` [`fallback::add_usage`] `` | `` [`crate::exec::fallback::add_usage`] `` |
| 183 | `` [`RECENT_OUTPUT_LINE_CHARS`] `` | `` [`crate::exec::RECENT_OUTPUT_LINE_CHARS`] `` |
| 288 | `` [`RunOptions::child_index`] `` | `` [`crate::exec::RunOptions::child_index`] `` |
| 299 | `` [`apply_thinking_suffix`] `` | `` [`crate::exec::apply_thinking_suffix`] `` |

(Line 46 also has `[`AgentProgress::snapshot`]` — leave as-is, `AgentProgress` is defined in this
same file. Line 53's two links are separated by `/` in the prose — `` .../[`completion_guard::has_mutation_tool_call`]/[`evaluate_completion_mutation_guard`] `` — fix both.)

### `exec/spawn_plan.rs` (4 sites)

| Line | Broken link | Fix |
|---|---|---|
| 34 | `` [`SpawnedChildAttemptRunner`] `` | `` [`crate::exec::attempt_runner::SpawnedChildAttemptRunner`] `` |
| 1015 | `` [`run_sync`] `` | `` [`crate::exec::run_sync`] `` |
| 1039 | `` [`validate_file_only_requires_path`] `` | `` [`crate::exec::output::validate_file_only_requires_path`] `` |
| 1044 | `` [`AgentDefinition`] `` | `` [`crate::discovery::types::AgentDefinition`] `` |

(Line 34 also has `[`ChildSpawnSpec`]` on the same line — leave as-is, it's imported locally via
`use crate::spawn::{ChildSpawnSpec, SpawnCommand};` at the top of this file and already resolves.
Line 1044 also has `` [`crate::exec::output::format_output_path_instruction`] `` — already correct,
leave as-is.)

### `formatters.rs` (1 site — wrong target, not just under-qualified)

| Line | Broken link | Fix |
|---|---|---|
| 26 | `` [`crate::background::ModelId`] `` | `` [`cyrup_core::ModelId`] `` |

## Verification

After applying all of the above:

```bash
cargo doc -p cyrup-ext-subagents --no-deps --lib
```

must exit 0 with no `unresolved link` errors. Also run:

```bash
cargo doc -p cyrup-ext-subagents --no-deps --lib 2>&1 | grep -c "unresolved link"
```

and confirm it prints `0`. Do not run a broader `cargo doc --workspace` as the pass/fail signal —
other crates may have pre-existing, out-of-scope issues; scope verification to `-p
cyrup-ext-subagents` as shown in the Evidence section above, matching how the regression was
originally confirmed (0 errors at merge-base, 58 at HEAD, all inside this crate).

## Non-goals

- Do not change any non-doc-comment code — every fix here is inside a `///` or `//!` line.
- Do not `pub`-ify `run_attempt`, `drive_attempt`, or anything else to "simplify" a path — the
  workspace already documents private items (`.cargo/config.toml`), so `pub(crate)`/private targets
  resolve fine once correctly qualified.
- Do not touch the six duplicate `format_tokens`/`format_model_thinking`/`run_mode_label`
  implementations mentioned in `formatters.rs`'s module doc — that's a separate, already-filed
  finding (`.flux/review/medium/formatters-module-dead-unwired.md` / `UNIFY_PATH_AND_CLOCK_HELPERS.md`), out of scope here.
