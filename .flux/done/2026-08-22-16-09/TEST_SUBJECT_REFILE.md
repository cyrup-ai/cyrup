---
stage: new
status: done
updated: 2026-08-22 19:32
---

# Redistribute The Round Grab-Bag Tests Into Subject Modules

**Crate:** `crates/cyrup-session-svc` · **Severity:** medium · **Effort:** large

## Description

After the renames and provenance work, six `roundN` modules plus `integration.rs` still file tests by review date instead of subject: round2 (408 lines, 9 tests), round3 (749, 16), round4 (373, 7), round6 (243, 4), round7 (260, 3), round9_l5res (740, 11) and integration.rs (749, 11) — around 80 tests in total. The concrete harm is already visible: compaction assertions are scattered across round2, round3, post_run_loop, round9_l5res, compact_refusals.rs and compaction_tokens_after.rs, so a reviewer touching compaction opens the two subject-named files and misses the rest; and integration.rs holds fork, session-list and trust tests that already have dedicated siblings at `src/tests/fork_non_persisted.rs`, `src/tests/session_list_dir.rs` and `src/tests/project_trust_extension.rs`. Because the round modules have no obvious subject, new tests land in whichever file the author had open, which is how round8_postrun acquired eight tests contradicting its own doc. This is the lowest value-per-effort item in the survey and is conflict-prone, so it is sequenced last and must land incrementally after both TEST_FIXTURE_COMMON_MODULE (so moved tests can use `super::common::*`) and TEST_PROVENANCE_AND_RENAMES.

## Acceptance Criteria

- [ ] `ls crates/cyrup-session-svc/src/tests | grep -c '^round'` returns 0; every moved test lives in a subject-named module, with new modules created only where no home exists.
- [ ] integration.rs's fork, session-list and trust tests are folded into the existing fork_non_persisted.rs, session_list_dir.rs and project_trust_extension.rs.
- [ ] The total test count under `src/tests/` is unchanged at 237 `#[test]`/`#[tokio::test]` items and `cargo test -p cyrup-session-svc` still reports 311 passing.
- [ ] No module doc in `src/tests/` enumerates more than two subjects, and `src/tests/mod.rs` remains alphabetically sorted.
- [ ] Moved tests use `super::common::{base_config, fixture, Fixture}` rather than re-pasted fixtures, and the work lands as at least three commits, one per source module group.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Refile the eight `roundN` test modules by subject — 69 of 237 tests are filed by review round, not by what they test

`CONFIRMED` · severity **medium** · effort **large** · dimension `test-organisation`

**Evidence.** `crates/cyrup-session-svc/src/tests/round2.rs` (408 lines, 9 tests), `round3.rs` (749, 16), `round4.rs` (373, 7), `round5.rs` (397, 7), `round6.rs` (243, 4), `round7.rs` (260, 3), `round8_postrun.rs` (652, 12), `round9_l5res.rs` (740, 11) — 3,822 lines and 69 of the directory's 237 tests. `round2.rs:1-4` lists seven unrelated subjects in one module doc; `round3.rs:1-4` lists nine. `round8_postrun.rs:1-6` promises post-run-loop proofs but 8 of its 12 tests are system-prompt discovery, session naming, websocket timeout, and user-tier agent loading. `integration.rs` (749 lines, 11 tests) holds fork, session-list, and trust tests that already have dedicated siblings `src/tests/fork_non_persisted.rs`, `src/tests/session_list_dir.rs`, `src/tests/project_trust_extension.rs`. `src/tests/mod.rs:22-26` is not alphabetised.

**Why it matters.** The `roundN` names encode a migration history nobody outside it can decode, so the directory index is only partly useful: a reviewer touching compaction opens `compact_refusals.rs` and `compaction_tokens_after.rs` and misses the compaction tests sitting in `round8_postrun.rs` and `round9_l5res.rs`. New tests then land in whichever file the author had open, which is demonstrably how `round8_postrun.rs` acquired eight tests that contradict its own doc comment — the doc is now actively misleading, which is the concrete harm here.

**Fix.** Move the 69 `roundN` tests plus `integration.rs`'s 11 into existing subject files where one exists (compaction → `compact_refusals.rs`/`compaction_tokens_after.rs`; `round5.rs`'s six `navigate_tree_*` → a new `tree_navigation.rs`; `round8_postrun.rs`'s system-prompt tests → `base_system_prompt.rs`; `integration.rs`'s fork/session-list/trust tests → `fork_non_persisted.rs`/`session_list_dir.rs`/`project_trust_extension.rs`), and create new subject files for the rest (`retry_and_postrun_loop.rs`, `model_and_thinking_control.rs`, `extension_input_event.rs`). Before moving, split each module-level gap-analysis header (e.g. `round9_l5res.rs:1-10`'s A.4/A.7/A.8 bullets) down onto the individual tests they refer to, so the provenance is not lost — that step is the bulk of the work, not the file moves. Sequence this after the `common` module lands so moved tests can `use super::common::*`. Retire the `roundN` filenames and alphabetise `src/tests/mod.rs` in the same pass.
