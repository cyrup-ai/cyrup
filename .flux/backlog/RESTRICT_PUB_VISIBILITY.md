---
stage: new
status: done
updated: 2026-08-23 02:15
---

# Default Module Items To pub(crate) So The Dead-Code Lint Can Work Again

## Description

The crate exports essentially everything: 1571 module-level `pub` items across 128 files, of which ~87% have no consumer outside the crate. rustc's `dead_code` lint stops at anything reachable from the crate root, so with everything `pub` the compiler is structurally unable to report unused code here — `cargo check -p cyrup-ext-subagents` finishes with zero warnings while 13 items are provably referenced nowhere in the workspace and a further ~53 are kept alive only by test code.

The consequences are already measurable and are part of this task, not separate ones. Thirteen `pub` items have zero references of any kind across the 1067 workspace .rs files, several carrying multi-line doc comments asserting a pi-upstream contract that makes them look like live seams (`resolve_skills` cites `skills.ts:608-638`; `AsyncJobsPayload` documents itself as 'the wire shape `cyrup-tui` deserializes' — a consumer the dependency graph forbids, since crates/cyrup-tui declares no dependency on this crate). One of them, `registration/slash_commands.rs:1636`, is a public function with an empty body. Another ~53 items have no production caller anywhere and survive only on `#[cfg(test)]` references — including two clusters that read as working subsystems but are not wired: the parent-side watchdog child-status ingest (`is_child_watchdog_status_event`/`child_watchdog_is_active`/`accept_child_watchdog_event`, 14 references, all in the file's own test module, while production only uses the emit half) and `watchdog/warning_format.rs`'s three formatters. A green suite asserting on `accept_child_watchdog_event` reads as proof the parent ingests child watchdog events; nothing does.

Making `pub` mean something again both shrinks the apparent contract and hands the compiler back the ability to catch this class of drift automatically.

## Evidence

Measured in /home/user/cyrup/crates/cyrup-ext-subagents: `grep -rnoE "^pub (async fn|fn|struct|enum|const|trait|type|static) \w+" src/ | wc -l` = 1571 across 128 files; `grep -rnoE '^pub\(crate\)' src/ | wc -l` = 214, so the narrower form is known and simply not reached for. Cross-referencing all 1571 names against every workspace .rs file: ~1360 (87%) have no non-comment reference outside the crate. `cargo check -p cyrup-ext-subagents` emits 0 warnings, and there is no blanket suppression (the only `allow(dead_code)` is a `cfg_attr(not(test), ...)` at background/tracker.rs:591). Dependents are exactly cyrup (Cargo.toml:67), cyrup-intercom (:30), cyrup-permission-system (:36), cyrup-it (:102). `grep -rn 'cyrup_ext_subagents::watchdog' --include=*.rs crates/ --exclude-dir=cyrup-ext-subagents` = 0 hits, while src/watchdog/ is 18,153 lines / 223 module-level pub items; `jsonl`'s only external mention is a doc comment. The 13 zero-reference items (each verified with word-wise grep across crates/, excluding its own definition and comment lines): background/control.rs:1332 `write_steer_ack`, discovery/skills.rs:149 `resolve_skills`, discovery/agent_memory.rs:435 `build_agent_memory_injection_for`, native_supervisor.rs:58 `NATIVE_SUPERVISOR_EXTENSION_DIR` and :367 `read_child_metadata`, registration/slash_commands.rs:1636 `pub fn parse_no_args_command(_raw_args: &str) {}`, spawn/nested_events.rs:1295/:1621/:1656/:1677, spawn/worktree.rs:54 `DEFAULT_HOOK_TIMEOUT`, tui/events.rs:834 `AsyncJobsPayload`, tui/render.rs:236 `fold_nested_summaries`. `grep -c ext-subagents crates/cyrup-tui/Cargo.toml` = 0. Test-only cluster examples: watchdog/child_status.rs:480/:497/:516 with all 14 references at lines 700-828, past the file's only `#[cfg(test)]` at :546; exec/mod.rs:1524 `build_attempt_spawn_plan` (all call sites past the `#[cfg(test)]` at :4575) versus the sole production call to `build_attempt_spawn_plan_with_read_requirement` at exec/mod.rs:2486. Note: cyrup-it's tests name ~101 distinct `cyrup_ext_subagents::` paths, so the surface that must stay `pub` is larger than the ~26 items production names.

## Suggested approach

Flip the default per module rather than crate-wide in one commit. Start with `pub mod watchdog` and `pub mod jsonl` in src/lib.rs, which have zero external references of any kind — narrowing them immediately puts 223 watchdog items under the dead_code lint and will surface much of the test-only set for free. Then do artifacts, discovery, missions and native_supervisor, checking cyrup-it's ~101 qualified paths before each (a `grep -o 'cyrup_ext_subagents::[a-z_:]*' crates/cyrup-it | sort -u` gives the keep-list). Delete the 13 unreferenced items first — that is a small independent commit. Triage the test-only set in two buckets: wrappers over a live `_with_X` variant (repoint the tests, delete the wrapper) and genuinely unwired subsystems (wire or delete).

## Acceptance Criteria

- [ ] `grep -rnoE "^pub (async fn|fn|struct|enum|const|trait|type|static) \w+" src/ | wc -l` drops below 400
- [ ] `pub mod watchdog` and `pub mod jsonl` in src/lib.rs are `pub(crate) mod` (or the modules' items are individually narrowed), and `cargo check -p cyrup-ext-subagents` then reports dead_code warnings that were previously invisible
- [ ] All 13 zero-reference items are deleted; grepping each name word-wise across /home/user/cyrup/crates (excluding target/) returns 0 hits
- [ ] `grep -n 'parse_no_args_command' src/registration/slash_commands.rs` returns nothing, including the referencing comment at :1681
- [ ] The watchdog child-status ingest trio and the warning_format trio are each either wired to a production caller (grep shows a call site outside any `#[cfg(test)]` region) or deleted along with their tests
- [ ] Test-convenience wrappers whose production twin is the `_with_X` variant (`build_attempt_spawn_plan`, `create_chain`, `request_async_steer`, `run_verify_command`) are either deleted with their tests repointed at the production entry point, or explicitly kept with a comment stating the upstream-parity reason (exec/mod.rs:1548-1557 already argues this for one of them)
- [ ] `cargo test -p cyrup-ext-subagents` passes with no new failures; `cargo check -p cyrup-ext-subagents` and `cargo build -p cyrup-it --tests` both still compile

## Source

- Identified by the `subagents-hygiene-survey` workflow (13 agents, 21 raw findings, 16 confirmed after adversarial verification).
- Effort: large · survey priority: 2 of 6
