---
stage: new
status: done
updated: 2026-08-23 00:06
---

# Break up the 9 functions over 500 lines (SessionBuilder::build 1,177; main.rs run 1,099) and the 30 too_many_arguments signatures (worst: 25 parameters)

> Found by an eight-lens workspace hygiene sweep. Every count below was reproduced
> against the tree before this task was filed.
> **Priority:** medium · **Effort:** large
> **Crates:** `cyrup-session-svc`, `cyrup`, `cyrup-ext-subagents`, `cyrup-resources`, `cyrup-intercom`, `cyrup-mcp`, `cyrup-agent`, `cyrup-tui`, `cyrup-tools`, `cyrup-provider`, `cyrup-ext`, `cyrup-config`

Two forms of the same problem — orchestration entry points that resist review and testing — measured independently of file size.

**Nine functions exceed 500 lines of body** (brace-balanced scan over all 1,291 non-test .rs files, inline `#[cfg(test)]` modules skipped; the top boundaries were hand-verified with `sed`, and I re-confirmed the two largest in this repo):

```
1177  crates/cyrup-session-svc/src/builder.rs:595-1771   fn build
1099  crates/cyrup/src/main.rs:94-1192                   fn run
 937  crates/cyrup-ext-sdk/src/example.rs:79-1015        fn build
 823  crates/cyrup-ext-subagents/src/exec/mod.rs:3653-4475  fn run_sync
 716  crates/cyrup-ext-subagents/src/exec/mod.rs:1558-2273  fn build_attempt_spawn_plan_with_read_requirement
 644  crates/cyrup-resources/src/discovery.rs:810-1453   fn discover_blocking
 589  crates/cyrup-intercom/src/tools/intercom.rs:137-725 fn dispatch
 539  crates/cyrup-mcp/src/proxy.rs:3059-3597            fn execute_call
 529  crates/cyrup-ext-subagents/src/background/runner_main.rs:1201-1729  fn run_inner
```

`main.rs::run` is **39%** of its 2,828-line file; `builder.rs::build` is **43%** of its 2,767-line file (both file sizes verified). Their host crates already have the sibling modules to receive the extracted phases: `crates/cyrup/src/` already contains `session_resolve.rs`, `startup_ui.rs`, `diagnostics.rs` and `run.rs`, and main.rs declares **zero** structs/enums/impls — it is 36 free functions that cluster cleanly into session resolution (`resolve_session`, `resolve_scoped_models_reporting`, `is_fresh_target`, `pick_scoped_active_model`, `gather_session_scopes/refs`, `list_global_sessions`, `session_list_layout/cwd_filter`, `print_resume_hint`), trust, interactive UI (`run_interactive`, `build_startup_report`, `build_theme_watcher`, `seed_footer`, `resolve_startup_ui`) and diagnostics. `builder.rs` is likewise three fused things: the SessionConfig/SessionTarget/NoTools/ExtensionFlagValue type model (:49-373), a 12-method fluent setter chain (:437-594), and the 1,177-line `build()` (:595-1771), then free helpers at :1838-2292. `crates/cyrup-resources/src/discovery.rs` (1,987 lines) sits in an already-decomposed crate (it has `package/` and `tests/` dirs) with `discover_blocking` doing all filesystem walking, package-tree resolution, override application, collision detection and report assembly in one unit.

**Thirty `#[allow(clippy::too_many_arguments)]` attributes across 12 of 21 crates** (verified count: 30) — the most-suppressed non-panic-policy lint in the workspace, and every one marks a real signature above the 7-arg threshold. By crate: cyrup-ext-subagents 5, cyrup-intercom 3, cyrup-session/cyrup-mcp/cyrup-agent 2 each, and 1 each in cyrup-tui, cyrup-tools, cyrup-session-svc, cyrup-resources, cyrup-provider, cyrup-ext, cyrup-config. Worst signatures: `run_or_background_graph` **25 params** (`cyrup-ext-subagents/src/extension/executor/chain.rs:221`), `RunState::new` **18** (`cyrup-agent/src/agent/run/mod.rs:83`), `run_chain_foreground` 13 (chain.rs:51), `run_chain_foreground_with_control` 12 (chain.rs:96), `run_compaction_prepared` and `finish_compaction` 12 each (`cyrup-session/src/compaction/mod.rs:176,207`), then 11-arg entries in `cyrup-resources/src/discovery.rs:1669`, `cyrup-mcp/src/oauth.rs:2875`, `cyrup-session-svc/src/session/mod.rs:310`, `cyrup-intercom/src/session_state.rs:729`, and 15 more at 8-9 args.

Both halves are mechanical and land incrementally, one function at a time. Sequence after WORKSPACE_FMT_BASELINE.

## Acceptance Criteria

- [ ] No function in the workspace (excluding test modules) exceeds 300 lines of body — verified by re-running the brace-balanced scan; in particular SessionBuilder::build, cyrup/src/main.rs::run, exec/mod.rs::run_sync and discovery.rs::discover_blocking are each split into named private phase functions
- [ ] cyrup/src/main.rs's free functions are moved into the sibling modules that already exist for them (session_resolve.rs, startup_ui.rs, diagnostics.rs, run.rs) and main.rs shrinks to a thin entry point
- [ ] The 25-argument `run_or_background_graph` (cyrup-ext-subagents/src/extension/executor/chain.rs:221) and the 18-argument `RunState::new` (cyrup-agent/src/agent/run/mod.rs:83) take a parameter struct or builder instead, and their #[allow(clippy::too_many_arguments)] attributes are deleted
- [ ] `grep -rn 'clippy::too_many_arguments' --include='*.rs' crates | wc -l` drops from 30 to at most 5, and every survivor carries a `reason = "..."` explaining why the signature cannot be grouped
- [ ] Each extraction is behavior-preserving: no change to public signatures other than the deliberate parameter-struct conversions, and every call site is updated in the same commit
- [ ] `cargo test --workspace` passes with no new failures, and `cargo clippy --workspace --all-targets` reports no new warnings

## Verifying command

```bash
cd /home/user/cyrup && sed -n '595p;1770,1771p' crates/cyrup-session-svc/src/builder.rs && sed -n '94p' crates/cyrup/src/main.rs && wc -l crates/cyrup/src/main.rs crates/cyrup-session-svc/src/builder.rs crates/cyrup-resources/src/discovery.rs && grep -rn 'clippy::too_many_arguments' --include='*.rs' crates | wc -l
```
