---
stage: new
status: done
updated: 2026-08-22 23:52
---

# Split src/tests/compaction.rs (2769 lines / 46 tests) and parity.rs (1308 / 38) into directory modules and extract the 4 copy-pasted fixtures into src/tests/fixtures.rs

> Found by a six-lens hygiene audit of `crates/cyrup-session`, run after the `manager/`
> decomposition landed in PR #53. Every claim below was reproduced against the tree.
> **Priority:** high · **Effort:** large

Two files are 4077 of the 6269 lines of test code in cyrup-session (65%). Both are already banner-partitioned by their authors and both partitions are **exact** — grouping the banners yields contiguous ranges whose test counts sum to the file totals with no test straddling a seam (re-verified by script, output below). This mirrors the `manager.rs` → `manager/` decomposition merged in PR #53 (`src/manager/` is now 9 files).

## A. `src/tests/compaction.rs` → `src/tests/compaction/` (mod.rs + 10 files)

25 `// ----- NAME -----` banners; verified partition:

```
   1- 208   208 ln   0 tests  fixtures.rs
 209- 738   530 ln  11 tests  acceptance.rs      A-05-1..A-05-10
 739- 927   189 ln   4 tests  tokens.rs          G-1 tokensBefore, M1, M2, G-3/G-8
 928-1078   151 ln   4 tests  keep_recent.rs     SESS-002
1079-1491   413 ln   8 tests  agent_messages.rs  THEME F, F-1, F-2, F-3
1492-1723   232 ln   3 tests  usage.rs           F-4
1724-1809    86 ln   2 tests  nested.rs          F-5
1810-2281   472 ln   7 tests  streaming.rs       F-6
2282-2512   231 ln   4 tests  guards.rs          StopReason::Pending + G-21
2513-2769   257 ln   3 tests  area03.rs          area-03 2026-08-14 pass
                        TOTAL 46 tests (file has 46)
```

Exactly three helpers defined outside the fixtures block cross a seam and must be hoisted into `compaction/fixtures.rs`: `RecordingArc` (def line 386; used by acceptance + area03), `custom_message_entry` (def 930; keep_recent + agent_messages), `branch_summary_entry` (def 944; keep_recent + agent_messages). Everything else localizes cleanly (`usage_of`/`UsageSummarizer`/`compaction_line` are F-4-only; `OptionSpy`/`f6_session`/`f6_settings`/`transient_failure_step`/`quota_failure_step` are F-6-only; `agent_msg_entry`/`bash_msg`/`CapturingHooks` are agent_messages-only; `compaction_entry_with_details`/`prev_details` are guards-only; `f3_session` is F-3-only).

## B. `src/tests/parity.rs` → `src/tests/parity/` (mod.rs + 7 files)

This file literally restarts mid-way: line 446 opens `// ===== round-2 gaps =====` and **lines 449-450 are a second `use` block at column 0**, 400+ lines below the file's real import header (`grep -n '^use ' parity.rs` → 5,7,8,9,10,11,12,17,21, **449, 450**). Line 722-724 opens a third round. Verified partition:

```
   1-  53    53 ln   0 tests  fixtures.rs
  54- 445   392 ln  13 tests  round1.rs      gaps 1,3/4,5,6,12..20
 446- 721   276 ln   9 tests  lifecycle.rs   round-2 gaps 12,21-24 + M3/M4  <- has its own use header
 722-1009   288 ln   8 tests  round3.rs      G-1..G-7
1010-1090    81 ln   4 tests  layout_open.rs gap-analysis 05 F-1..F-3
1091-1177    87 ln   2 tests  sess001.rs
1178-1308   131 ln   2 tests  sess015.rs
                        TOTAL 38 tests (file has 38)
```

Two cross-seam helpers to hoist: `assistant_text` (def 304, used by round1 + sess001) and `asst` (def 454, used by round1/round3/layout_open/lifecycle — currently forward-referenced from 150 lines above its definition).

## C. `src/tests/fixtures.rs` — kill the copy-paste

`src/tests/mod.rs` is 11 lines and declares 8 sibling modules with no shared fixture module, so the same helpers are duplicated. Byte-identical, verified by md5 of the exact source ranges:

- `fn user(s: &str) -> Message` — md5 `6ef91b2d197ab9f973962d942348e584`, **4 copies**: `compaction.rs:50`, `parity.rs:23`, `sessions.rs:19`, `area03_repairs.rs:20`
- `fn assistant(s: &str)` — `compaction.rs:50-71` and `sessions.rs:19-40` diff to empty (22 identical lines)
- `fn layout(root, cwd)` — md5 `73b60b631181854b8c0d3a594a08f4e6`, 2 copies: `compaction.rs:42`, `sessions.rs:15`
- the same 13-line first-text extractor — md5 `47fd32e7b83b7ffec95f897088cbddcf`, **3 copies under 2 names**: `compaction.rs:195 first_text`, `sessions.rs:41 first_text`, `parity.rs:27 text_of`

~60 redundant lines plus a naming fork. Add `src/tests/fixtures.rs`, declare `mod fixtures;` in `tests/mod.rs`, keep `first_text` as the single name (2 of 3 sites already use it), drop `text_of`. Helpers unused by some modules will trip `dead_code` — put `#![allow(dead_code)]` at the top of `fixtures.rs` or import per-module rather than glob.

## Constraint

Directory modules preserve the single-test-binary property that `tests/mod.rs:1-2` exists for ("relocated from `tests/` so the whole suite links into ONE binary"). `cargo test -p cyrup-session --no-run` must still build exactly one executable. Assertions move verbatim; the only edits are `mod` declarations, the hoisted helpers, and the fixture imports.

## Acceptance Criteria

- [ ] `src/tests/compaction.rs` and `src/tests/parity.rs` no longer exist as files; `src/tests/compaction/` and `src/tests/parity/` exist with the file breakdown above, and no file in `src/tests/` exceeds 600 lines
- [ ] `src/tests/fixtures.rs` exists, is declared in `src/tests/mod.rs`, and defines a single canonical `user`/`assistant`/`layout`/`first_text`; `grep -rn 'fn text_of' src/tests/` returns nothing
- [ ] `cargo test -p cyrup-session` reports the same pass count as before the split (157 passed, 0 failed) — no test silently dropped by a missing `mod` declaration
- [ ] `cargo test -p cyrup-session --no-run` still builds exactly one test executable (`unittests src/lib.rs`)
- [ ] `git diff --stat` shows no assertion text changed: every moved `assert*` line is identical to its pre-split form
- [ ] `cargo clippy --all-targets -p cyrup-session` reports 0 findings

## Verifying command

```bash
cd /home/user/cyrup && wc -l crates/cyrup-session/src/tests/*.rs crates/cyrup-session/src/tests/*/*.rs 2>/dev/null && cargo test -p cyrup-session 2>&1 | tail -3 && cargo test -p cyrup-session --no-run 2>&1 | grep -c 'Executable'
```
