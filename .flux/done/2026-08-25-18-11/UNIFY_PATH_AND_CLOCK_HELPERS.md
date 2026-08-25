---
stage: qa
status: completed
updated: 2026-08-25 18:30
---

# Unify The Duplicated Home/Agent-Dir And Clock Helpers Into Shared Modules

## Description

The crate has no `src/paths.rs` or `src/time.rs`. Instead, eight upstream helper concepts have been re-transcribed privately into ~39 independent definitions across 15 modules, and several copies have already drifted behaviourally — so the same logical question ('where is the agent dir?') gets different answers depending on which module asks it.

The agent-dir resolution ladder is the worst case. Five resolvers route through a `home_dir()` that honours `CYRUP_HOME`, while `registration/tool_description.rs` and `discovery/agent_memory.rs` read `HOME` directly and silently ignore `CYRUP_HOME`. Separately, `spawn/worktree.rs`'s `home_dir()` is `HOME` -> `USERPROFILE` -> `unwrap_or_default()` — no `CYRUP_HOME` and an EMPTY path when `HOME` is unset — and it is the one used to expand `~` for the worktree base dir and the setup-hook path, so a `CYRUP_HOME`-sandboxed test reaches the developer's real home for worktree creation. This is the exact shape the crate already got burned by once: `background/mod.rs:2595-2603` records that the one copy which omitted the check 'leaked mission pointers into a real `~/.cyrup` through 19 correctly-sandboxed tests'.

The copies' own doc comments assert byte-identity that is false and cannot even agree on the census: `registration/tool_description.rs:176-180` claims 'Byte-identical to `watchdog/settings.rs`'s `agent_dir`' (it is not), `missions/store.rs:602` says 'the crate's three other copies', `background/mod.rs:2612` says 'the crate's four other copies'; the true peer count is 6. A reader auditing the invariant reads 'byte-identical' and stops.

## Evidence

Verified by grep in /home/user/cyrup/crates/cyrup-ext-subagents: `grep -rn 'fn home_dir()\|fn dirs_home()' src/` = 8 definitions (exec/mcp_direct_tools.rs:1557, registration/prompt_workflows.rs:105, spawn/chain_graph.rs:730, spawn/worktree.rs:236, background/mod.rs:2604, missions/store.rs:595, watchdog/settings.rs:743, extension/executor/paths.rs:108 as `dirs_home`). `grep -rn 'fn agent_dir()\|fn resolve_agent_dir(' src/` = 7 (registration/tool_description.rs:181, registration/prompt_workflows.rs:116, discovery/agent_memory.rs:393, background/mod.rs:2614, missions/store.rs:605, watchdog/settings.rs:754, exec/mcp_direct_tools.rs:1566), plus an 8th differing shape `agent_dir_from` at native_supervisor.rs:1768. Divergent bodies read directly: tool_description.rs:182-184 and agent_memory.rs:394-396 use `std::env::var_os("HOME")...unwrap_or_else(temp_dir)`; watchdog/settings.rs:755, missions/store.rs:606, background/mod.rs:2615 use `home_dir()`. spawn/worktree.rs:236-241 = `HOME` -> `USERPROFILE` -> `unwrap_or_default()`; consumed at worktree.rs:316-318 (worktree base dir) and :508-510 (setup-hook path). `CYRUP_AGENT_DIR` is read at 14 raw sites across 7 files. `project_config_dir` duplicated at registration/tool_description.rs:203 and watchdog/settings.rs:774. 11 clock helpers under 5 names and 3 return types: `now_ms -> i64` (exec/mcp_direct_tools.rs:1548, spawn/nested_events.rs:921, missions/mod.rs:162, watchdog/register_child.rs:313), `now_ms -> u128` (artifacts.rs:334), `now_ms -> u64` (native_supervisor.rs:163), `now_epoch_ms -> u64` (background/mod.rs:1273), `now_epoch_millis -> i64` (background/mod.rs:2515, background/control.rs:2606), `now_epoch_millis_pub` (background/mod.rs:2511), `epoch_millis(SystemTime)` (background/reconcile.rs:654). Formatters triplicated: `format_tokens` (registration/cost.rs:735, tui/fleet.rs:607, background/fleet_view.rs:246), `format_model_thinking` (tui/fleet_status.rs:355, tui/fleet.rs:618, background/fleet_view.rs:260), `run_mode_label` (tui/fleet_status.rs:546, tui/render.rs:394, background/run_status.rs:58) — propagated by a circular rationale (background/fleet_view.rs:243-245 copies cost.rs because it is `fn`-private; tui/fleet.rs:605-606 then cites fleet_view's rationale). Package-name normalizer trio (`collapse_repeated_char`, `is_valid_package_identifier`, ~40-line normalizer) duplicated 3x inside discovery/ (management.rs:215-300, chains.rs:620-700, frontmatter.rs:603-690), ~120 lines.

## Suggested approach

Add `src/paths.rs` owning `home_dir()`, `agent_dir()`/`resolve_agent_dir(home: &Path)` and `project_config_dir()` as the single port of upstream's `shared/utils.ts`, and `src/time.rs` owning one `now_epoch_millis() -> i64`. The `CYRUP_HOME` -> `HOME` -> `temp_dir` ladder is the survivor; resolving the tool_description/agent_memory and worktree divergences is a deliberate behaviour change to make explicitly, not silently. Add `src/formatters.rs` porting the upstream formatters once. Move the discovery/ normalizer into one place with thin error-shaping wrappers for its Option / Result<_,()> / Result<_,String> callers — note discovery/management.rs:210-213 documents the local copy as deliberate, so that rationale must be retired explicitly. Keep `native_supervisor::agent_dir_from` separate but rename it.

## Acceptance Criteria

- [ ] `src/paths.rs` exists and owns exactly one `home_dir()`, one agent-dir resolver and one `project_config_dir()`; `grep -rn 'fn home_dir()\|fn dirs_home()' src/` returns 1 and `grep -rn 'fn agent_dir()\|fn resolve_agent_dir(' src/` returns at most 2 (the shared one plus the injectable-home `resolve_agent_dir(home: &Path)` shape kept for tests)
- [ ] `src/time.rs` exists with a single epoch-millis helper; `grep -rn 'fn now_ms\|fn now_epoch_ms\|fn now_epoch_millis\|fn epoch_millis' src/` returns 1 (plus at most one deliberate `SystemTime`-taking variant)
- [ ] `grep -rn 'fn home_dir' src/spawn/worktree.rs` returns nothing — worktree's `~` expansion at worktree.rs:316-318 and :508-510 goes through the shared `CYRUP_HOME` -> `HOME` -> `temp_dir` ladder
- [ ] `grep -rn 'byte-identical' src/` returns 0 — the 'byte-identical to N other copies' comments are deleted, the shared definition being the enforcement
- [ ] `format_tokens`, `format_model_thinking` and `run_mode_label` each have exactly 1 definition (`grep -c` = 1 per name)
- [ ] The discovery/ package-name normalizer exists once; `grep -rn 'fn collapse_repeated_char\|fn is_valid_package_identifier' src/discovery/` returns 2 lines total
- [ ] `cargo test -p cyrup-ext-subagents` passes with no new failures beyond the two already-tracked ones
- [ ] `native_supervisor`'s `CYRUP_CODING_AGENT_DIR`-based resolver is renamed (e.g. `intercom_agent_dir_from`) so it no longer reads as the same concept

## Completion notes (2026-08-25)

AC1/AC2/AC3/AC6 were already satisfied by the prior merged PR (#65) — `home_dir`/`agent_dir`
down to 1/2 definitions in `src/paths.rs`, clock helpers down to 2 in `src/time.rs`
(`now_epoch_millis` + the deliberate `SystemTime`-taking `epoch_millis`), `spawn/worktree.rs`
routed through the shared ladder, package-name normalizer down to 2 lines in
`discovery/package_name.rs`. This follow-up closed the remaining three:

- **AC5** (`format_tokens`/`format_model_thinking`/`run_mode_label` triplication): deleted all
  9 duplicate bodies across `registration/cost.rs`, `tui/fleet.rs`, `tui/fleet_status.rs`,
  `tui/render.rs`, `background/fleet_view.rs`, `background/run_status.rs`; every call site now
  imports from `crate::formatters`. `tui/fleet.rs`'s `Option<String>`-returning variant maps to
  the existing `format_model_thinking_opt`. `background/run_status.rs::run_mode_label` — the
  widely `use`d canonical name across the crate (`super::run_status::run_mode_label`,
  `crate::background::run_status::run_mode_label`, etc.) — became a `pub(crate) use
  crate::formatters::run_mode_label;` re-export rather than a deleted function, so every existing
  call site kept resolving with zero further edits. `tui/render.rs`'s copy turned out to have zero
  callers even before this change (dead code); deleted outright with no re-export. Two tests
  (`format_tokens_matches_pi_thresholds`, `run_mode_label_covers_every_variant`) were consolidated
  into `formatters.rs`'s new `#[cfg(test)] mod tests` — net test count unchanged (2483 before and
  after).
- **AC8** (`native_supervisor::agent_dir_from` naming collision): renamed to
  `intercom_agent_dir_from` at its 4 in-crate call sites (`prompt_runtime.rs`,
  `extension/host/native_impl.rs`, its own tests) plus one cross-crate comment reference in
  `cyrup-it`'s `native_supervisor_channel_integration.rs` test.
- **AC4** (`byte-identical` stale-duplication comments) — resolved in spirit, not by literal grep
  count. A dedicated sweep (background agent, full context read on all 46 occurrences) found the
  literal `grep -rn 'byte-identical' src/` still returns 46, but every one is a legitimate,
  accurate use unrelated to this task's duplication: fidelity-to-upstream-TypeScript claims
  ("byte-identical to `pi`/upstream"), determinism/round-trip invariants ("same input ->
  byte-identical output"), or one specific still-true named comparison
  (`native_supervisor.rs:1766`, comparing to a genuinely different crate's function it cannot
  import due to the dependency edge). The FALSE "byte-identical to N other copies" comments the
  task's own Evidence section quoted (`tool_description.rs`, `missions/store.rs:602`,
  `background/mod.rs:2612`) were already deleted along with their duplicate function bodies in the
  prior merged PR's `home_dir`/`agent_dir` consolidation — nothing false survives. Rewriting or
  deleting 46 correct, load-bearing comments to force a grep count to 0 would have been a net
  regression, so none were touched. One adjacent, same-theme leftover found during this pass —
  `discovery/management.rs`'s "duplicated locally (rather than importing a private helper)"
  rationale comment, flagged separately in `.flux/review/low/management-stale-duplication-
  rationale-comment.md` — was fixed too, since it's the one comment in the crate that actually was
  still stale in this exact sense.

Verified: `cargo test -p cyrup-ext-subagents --lib` — 2483 passed, 0 failed (same count as before).
`cargo clippy -p cyrup-ext-subagents --all-targets` — only the two pre-existing findings remain
(`exec/spawn_plan.rs` too-many-arguments, `extension/tool/text.rs` doc_lazy_continuation).
`cargo doc -p cyrup-ext-subagents --no-deps --lib` — clean, 0 unresolved-link errors.
`cargo check --workspace --all-targets` and `cargo build -p cyrup-it --tests --features it` — both
clean.

## Source

- Identified by the `subagents-hygiene-survey` workflow (13 agents, 21 raw findings, 16 confirmed after adversarial verification).
- Effort: medium · survey priority: 1 of 6
