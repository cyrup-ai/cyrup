---
stage: new
status: done
updated: 2026-08-23 00:00
---

# Rename The Single-Subject Round Modules And Push Provenance Onto Tests

**Crate:** `crates/cyrup-session-svc` · **Severity:** medium · **Effort:** medium

## Description

Eight `roundN` modules in `src/tests/` hold 3,822 lines and 69 of the directory's 237 tests — 25% of the test corpus filed by review round rather than subject, sitting beside 38 subject-named siblings. Two of the eight are already single-subject per their own module docs and are pure renames: `round5.rs` (397 lines) is entirely `/tree` navigation and `round8_postrun.rs`'s first four tests are the post-run loop it advertises. The blocking work for the larger refile is that provenance currently lives in module-level headers — `round9_l5res.rs:1-10` lists gap-analysis items A.4/A.7/A.8 for the whole file, `round3.rs:1-4` enumerates eight unrelated subsystems, `round2.rs:1-4` seven — and `rg 'gap-analysis' round*.rs` finds nothing at test level, so moving tests would silently discard why each exists. This session does the rename plus the provenance redistribution in place, with no test bodies moved, leaving the redistribution mechanical. `src/tests/mod.rs` is also out of sort order at :22-23 (get_commands_source_info before fork_parent_and_unsaved_guard) and :25-26 (late_seams before integration).

## Acceptance Criteria

- [x] `git mv` renames round5.rs → navigate_tree.rs and round8_postrun.rs → post_run_loop.rs; `ls crates/cyrup-session-svc/src/tests | grep -c '^round'` returns 6.
- [x] Every bullet in the six remaining round modules' doc headers is attached as a `///` comment to the specific test(s) it describes; each module doc retains at most a one-line pointer.
- [x] The `mod` lines in `src/tests/mod.rs` are alphabetically sorted — verified by extracting them and running `sort -c`.
- [x] `git diff` shows no changes inside test function bodies (renames, `mod` declarations and comments only).
- [x] `cargo test -p cyrup-session-svc` still reports 311 passing and `cargo clippy --all-targets` gains no warnings.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Rename the eight `roundN` test modules to subject names; they are review-round grab-bags, not test suites

`CONFIRMED` · severity **medium** · effort **large** · dimension `consistency`

**Evidence.** src/tests/mod.rs:35-42 declares round2..round9_l5res — 3,822 lines, 69 tests, 25% of the 15,100-line test corpus. Module docs confirm the grab-bag: src/tests/round3.rs:1-4 lists eight unrelated subsystems (retry, auto-compaction, immediate-bash, dynamic/custom tools, setModel/cycleModel, prompt ordering + skill expansion, clone_at, modelFallbackMessage); round2.rs:1-4 and round4.rs likewise. The other 38 modules in the same directory are subject-named (compact_refusals, fork_non_persisted, mid_run_tool_anchoring, settings_resolve, ...), so two schemes sit side by side. late_seams.rs (393L, 3 tests) has the same problem under a different label. src/tests/mod.rs:22-23 and :25-26 are also out of sort order.

**Why it matters.** Coverage for one subsystem is scattered by review date rather than by subject — compaction assertions live across round2, round3, round8_postrun, round9_l5res, compact_refusals and compaction_tokens_after — so a failing module name tells a reviewer only which review round wrote the assertion. New tests have no obvious home, which is how the round files kept accreting unrelated topics.

**Fix.** Start with the cheap, safe half: `git mv` round5.rs -> navigate_tree.rs and round8_postrun.rs -> post_run_loop.rs (both are already single-subject per their own module docs), and sort src/tests/mod.rs while touching it. Then, incrementally, split the remaining six along the subsystem boundaries their module docs already enumerate and fold the pieces into the existing subject-named modules (retry/auto-compaction into compaction_*, the assembled-run proofs into agent_settled, etc.). Do not attempt all eight in one commit.
