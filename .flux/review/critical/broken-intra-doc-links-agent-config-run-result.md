---
severity: critical
file: crates/cyrup-ext-subagents/src/exec/agent_config.rs
lines: 33-626
introduced: true
---

# Split leaves ~29 intra-doc links unresolved, failing the workspace's `deny(rustdoc::broken_intra_doc_links)` gate

## Problem
`agent_config.rs` and `run_result.rs` were split out of `exec/mod.rs` with their doc comments
carried over **verbatim** (confirmed byte-for-byte identical to the pre-split content via diff
against `ebf328c87830a5ebf84a2c48f640e5aa7602cee9`). Many of those doc comments use bare,
unqualified intra-doc link paths — `[`run_sync`]`, `[`SingleResult`]`, `[`RunOptions::clarify`]`,
`[`fallback::ModelAttempt`]`, `[`output::OutputCap`]`, `[`completion_guard::...`]`,
`[`PARENT_SESSION_ENV_VAR`]`, `[`drive_attempt`]`, `[`plan_batch`]`, `[`build_task_text`]`,
`[`build_attempt_spawn_plan`]`, `[`apply_thinking_suffix`]`, `[`RECENT_OUTPUT_CAP`]`,
`[`RECENT_OUTPUT_LINE_CHARS`]`, `[`tool_call_summary::ToolCallSummary`]` — that only resolved
because the text used to live directly inside `exec/mod.rs`, where `run_sync`, `plan_batch`,
`build_task_text`, `apply_thinking_suffix`, `RunOptions` and `SingleResult` were defined, and
`fallback`, `output`, `completion_guard`, `acceptance`, `tool_call_summary` were declared as child
`mod`s (all directly nameable, unqualified, from that module's own scope), and
`PARENT_SESSION_ENV_VAR` reached scope via `mod.rs`'s own `pub use spawn_plan::*;` glob.

`agent_config.rs` and `run_result.rs` are now separate modules (`crate::exec::agent_config`,
`crate::exec::run_result`). Rust modules do not inherit a parent module's local scope, so every one
of these bare paths now fails to resolve from the new files. The workspace pins
`[workspace.lints.rustdoc] broken_intra_doc_links = "deny"` (`Cargo.toml`, labeled "rustdoc lints
(regrowth gate)"), which every crate inherits via `[lints] workspace = true` — so this is a hard
build failure for `cargo doc`, not a lint warning to clean up later.

## Evidence
`cargo doc -p cyrup-ext-subagents --no-deps --lib` fails with, among others (all newly-broken by
this split — confirmed by diffing the struct/impl bodies against the merge-base `exec/mod.rs`,
which are byte-identical apart from these files' new module boundary):

```
error: unresolved link to `completion_guard::expects_implementation_mutation`
  --> crates/cyrup-ext-subagents/src/exec/agent_config.rs:33:56
   | no item named `completion_guard` in scope

error: unresolved link to `run_sync`
   --> crates/cyrup-ext-subagents/src/exec/agent_config.rs:308:32
   | no item named `run_sync` in scope

error: unresolved link to `SingleResult`
   --> crates/cyrup-ext-subagents/src/exec/agent_config.rs:356:47
   | no item named `SingleResult` in scope

error: unresolved link to `PARENT_SESSION_ENV_VAR`
   --> crates/cyrup-ext-subagents/src/exec/agent_config.rs:419:11
   | no item named `PARENT_SESSION_ENV_VAR` in scope

error: unresolved link to `RunOptions::include_progress`
  --> crates/cyrup-ext-subagents/src/exec/run_result.rs:15:52
   | no item named `RunOptions` in scope

error: unresolved link to `RECENT_OUTPUT_CAP`
   --> crates/cyrup-ext-subagents/src/exec/run_result.rs:168:11
   | no item named `RECENT_OUTPUT_CAP` in scope

error: could not document `cyrup-ext-subagents`
```

Full list of newly-broken links:

`agent_config.rs`: lines 33 (`completion_guard::expects_implementation_mutation`), 34
(`acceptance::AcceptanceContract::heuristic_default`), 40 (`apply_thinking_suffix`), 77
(`output::OutputCap`), 304 (`fallback`), 308/323/346/386/548 (`run_sync`, 5×), 335
(`build_task_text`), 356/392 (`SingleResult`, 2×), 363 (`build_attempt_spawn_plan`), 387
(`SingleResult::progress`), 408 (`plan_batch`), 419 (`PARENT_SESSION_ENV_VAR`), 427
(`drive_attempt`), 521 (`SingleResult::control_events`), 621 (`fallback::ModelAttempt`), 625
(`SingleResult::tool_calls`), 626 (`tool_call_summary::ToolCallSummary`).

`run_result.rs`: lines 15/149 (`RunOptions::include_progress`, 2×), 39 (`drive_attempt`), 41
(`RunOptions::clarify`), 136 (`output::truncate_output`), 168 (`RECENT_OUTPUT_CAP` and
`RECENT_OUTPUT_LINE_CHARS`).

## Impact
`cargo doc -p cyrup-ext-subagents` (and any workspace-wide `cargo doc`) now hard-fails to build
because of `deny(rustdoc::broken_intra_doc_links)`. Given the repo's own comment calling this a
"regrowth gate," this almost certainly breaks a CI/merge check, blocking the PR (or any later PR
that touches this crate) independent of runtime correctness. It also silently degrades the
generated docs for every reader who does get a build through (e.g. via `--cap-lints warn`),
since none of these cross-references render as links any more.

## Suggested Fix
Requalify each bare path to a full `crate::exec::...` path (matching the style this same PR already
uses for cross-module references like `crate::exec::acceptance::AcceptanceContract` and
`crate::exec::fallback::ModelOverride`), e.g.:
- `[`run_sync`]` → `[`crate::exec::run_sync`]`
- `[`SingleResult`]` → `[`crate::exec::run_result::SingleResult`]` (or a relative `[`super::run_result::SingleResult`]`/`[`crate::exec::SingleResult`]` via the `pub use` re-export)
- `[`RunOptions::...`]` → `[`crate::exec::agent_config::RunOptions::...`]` (or `[`super::RunOptions::...`]` from `run_result.rs`)
- `[`fallback`]`/`[`fallback::ModelAttempt`]`/`[`output::OutputCap`]`/`[`output::truncate_output`]`/`[`completion_guard::...`]`/`[`acceptance::...`]`/`[`tool_call_summary::ToolCallSummary`]` → `crate::exec::fallback`/`crate::exec::output`/`crate::exec::completion_guard`/`crate::exec::acceptance`/`crate::exec::tool_call_summary` equivalents
- `[`PARENT_SESSION_ENV_VAR`]` → `[`crate::exec::PARENT_SESSION_ENV_VAR`]`
- `[`plan_batch`]`, `[`build_task_text`]`, `[`build_attempt_spawn_plan`]`, `[`apply_thinking_suffix`]`, `[`drive_attempt`]`, `[`RECENT_OUTPUT_CAP`]`, `[`RECENT_OUTPUT_LINE_CHARS`]` → their actual `crate::exec::...` (or `crate::tui::events::...`) homes

Then run `cargo doc -p cyrup-ext-subagents --no-deps --lib` (workspace lints already `deny` this)
to confirm zero `unresolved link` errors remain in these two files.
