---
stage: new
status: done
updated: 2026-08-22 19:32
---

# Add A Shared Test Fixture Module And One Lint Waiver

**Crate:** `crates/cyrup-session-svc` · **Severity:** high · **Effort:** medium

## Description

`src/tests/` is 49 files and declares 48 leaf modules with no shared support module, so ~764 lines of fixture boilerplate are copy-pasted: `struct Fixture` appears in 41 files (39 byte-identical, md5 de2fb31b), `fn fixture()` in 38 byte-identical copies, and `fn base_config` in 35 copies across 5 variants — one of which (`late_seams.rs`'s `no_extensions = true`) is deliberate and the rest are unmarked drift. The same files each re-paste the lint waiver: 45 byte-identical `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` lines plus a 3-lint variant in `delete_session_file_trash.rs` that has already dropped `indexing_slicing`; three files carry none and compile fine. A separate audit of the 10 inline `#[cfg(test)] mod` allows in production files (provider_swap.rs:99, guest_providers.rs:77, state.rs:351, builder.rs:2345, bash.rs:584 and :714, hooks.rs:341, host_services.rs:1879, tools.rs:310, attribution.rs:148) confirmed every one sits directly on a test module and no production code hides a denied lint — but they use six different lint subsets. Both halves of the fix were validated empirically: one inner attribute in `src/tests/mod.rs` suppresses the denies in every out-of-line child module, and `cargo clippy --all-targets` stayed error-free after deleting per-file headers.

## Acceptance Criteria

- [ ] `src/tests/common.rs` exists, is declared first in `src/tests/mod.rs`, and exports `pub(super)` `Fixture`, `fixture()`, `base_config()` plus a `base_config_no_extensions` variant for the 8-file case.
- [ ] `rg -l 'struct Fixture' crates/cyrup-session-svc/src/tests | wc -l` returns at most 3 — common.rs plus the two genuinely divergent copies (read_image_auto_resize.rs, round9_l5res.rs), each renamed so the divergence is visible.
- [ ] `rg '^#!\[allow\(clippy::' crates/cyrup-session-svc/src/tests/*.rs` matches exactly one line, in mod.rs, carrying a one-line comment stating the policy; the commit message states the deliberate trade-off that delete_session_file_trash.rs gains `indexing_slicing`.
- [ ] The 10 inline `#[cfg(test)] mod tests` allows in `src/` proper are normalised to the same 4-lint inner form, or the PR states which are deliberately narrower and why.
- [ ] `cargo clippy -p cyrup-session-svc --all-targets` reports no unwrap_used/expect_used/panic/indexing_slicing denials and `cargo test -p cyrup-session-svc` still reports 311 passing.
- [ ] `git diff --stat` on `src/tests/` shows a net deletion of at least 700 lines.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Add a shared `src/tests/common` module — ~760 lines of byte-identical fixture boilerplate are copied across 44 test files

`CONFIRMED` · severity **high** · effort **medium** · dimension `test-organisation`

**Evidence.** `crates/cyrup-session-svc/src/tests/mod.rs:1-51` declares 48 leaf `mod` lines and no shared support module. Identical 20-line helper blocks at `crates/cyrup-session-svc/src/tests/abort_settles.rs:42-61`, `crates/cyrup-session-svc/src/tests/round2.rs:22-41`, `crates/cyrup-session-svc/src/tests/session_dag.rs:21-40`, `crates/cyrup-session-svc/src/tests/late_seams.rs:27-46` (the last adds one `cfg.no_extensions = true;`). Clippy opt-out: 45 files carry `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` verbatim (e.g. `abort_settles.rs:27`, `round2.rs:5`), 1 file spells it without `indexing_slicing`, and 2 more use a multi-line form (`read_image_auto_resize.rs:14`, `settings_resolve.rs:15`) — 48 files in total, exactly as claimed.

**Why it matters.** Changing the fixture shape — adding a field, changing the temp-dir layout, changing the default `SessionConfig` — is a 44-file edit today. That cost is what makes contributors copy-paste the block into file 45 instead, so the duplication compounds. It also hides real divergence: five distinct `base_config()` bodies already exist and nothing marks which differences are deliberate (`late_seams.rs`'s `no_extensions = true`) versus accidental drift.

**Fix.** Add `src/tests/common.rs` with `pub(super) struct Fixture`, `pub(super) fn fixture()`, `pub(super) fn base_config(&Fixture) -> SessionConfig`, plus a `base_config_no_extensions` for the 8-file variant. Declare `mod common;` first in `src/tests/mod.rs` and replace each leaf copy with `use super::common::{base_config, fixture, Fixture};` (`pub(super)` on `tests::common` items is visible to every sibling leaf module — privacy extends to descendants of `tests`). Keep the two genuinely-divergent `fixture()` bodies local but rename them so the divergence is visible. Then delete the 48 per-file allow attributes and hoist one `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` into `src/tests/mod.rs` — verified to propagate to every child module. Do NOT justify the crate-local choice by a dependency cycle; `cyrup-test-support` is reachable as a dev-dependency (proven to compile). Justify it by internal visibility instead, and consider reusing `cyrup_test_support::TestTempDir` inside `common.rs` rather than reimplementing it.

### Collapse the 58 duplicated test-lint `allow` headers into one policy (audit result: no production code sidesteps a denied lint)

`CONFIRMED` · severity **medium** · effort **medium** · dimension `lint-debt`

**Evidence.** Reproduced: `rg -o 'allow\([^)]*\)' src | sort | uniq -c` → 48× `(unwrap_used, expect_used, panic, indexing_slicing)`, 3× `(unwrap_used, indexing_slicing)`, 3× `(unwrap_used, expect_used, indexing_slicing)`, 2× `(unwrap_used, expect_used, panic)`, 1× `too_many_arguments`; plus 2 multi-line 4-lint headers. `for f in src/tests/*.rs; do rg -q 'allow(' $f || echo $f; done` → only src/tests/mod.rs. /home/user/cyrup/crates/cyrup-session-svc/src/tests/delete_session_file_trash.rs:30 = `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` (no indexing_slicing). FIX VALIDATED EMPIRICALLY: I inserted `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` at src/tests/mod.rs:4 and deleted the header from src/tests/abort_settles.rs — `cargo clippy -p cyrup-session-svc --all-targets` still produced exactly the same 4 warnings, zero denied-lint errors from abort_settles. Files restored; `git status` clean.

**Why it matters.** A new file in src/tests/ that forgets the header fails to compile for a reason unrelated to what it tests — a papercut already paid 48 times. Four spellings of one policy mean nobody can tell whether the narrower forms (e.g. hooks.rs:341's 2-lint version) were deliberate or just what got pasted that day, and delete_session_file_trash.rs:30 proves the copy-paste already drifted. None of the 58 carries a rationale comment.

**Fix.** Put one inner attribute at the top of /home/user/cyrup/crates/cyrup-session-svc/src/tests/mod.rs — `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` with a one-line comment stating the policy — then delete the 48 per-file headers (lint levels on a module are lexically scoped over out-of-line child modules it declares; I verified this holds here). For the 10 inline `#[cfg(test)] mod` blocks, normalize on the inner `#![allow]` placement (matching host_services/tools/hooks/attribution) and the same 4-lint list.

### Collapse 46 duplicated `#![allow(clippy::...)]` headers in src/tests/ into one in mod.rs

`CONFIRMED` · severity **low** · effort **small** · dimension `manifest`

**Evidence.** `rg -l '^#!\[allow\(clippy::' crates/cyrup-session-svc/src/tests` → 46 of 49 files; `rg -N '^#!\[allow\(clippy::' --no-filename | sort | uniq -c` → 45 identical 4-lint lines + 1 three-lint line. crates/cyrup-session-svc/src/tests/mod.rs:1-51 has the doc comment and 48 `mod` lines, no lint attribute. Workspace denies the four lints at root Cargo.toml:97-101; the crate opts in at Cargo.toml:11-12. Fix applied and `cargo clippy -p cyrup-session-svc --all-targets` stayed error-free.

**Why it matters.** 46 copies of one waiver is 46 places a reviewer must re-read to confirm they are the same waiver, and it hides the one genuinely narrower waiver (delete_session_file_trash.rs, which omits indexing_slicing) in the noise. It also guarantees every new test file starts by copy-pasting a blanket lint escape.

**Fix.** Insert `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` immediately after the doc comment at crates/cyrup-session-svc/src/tests/mod.rs:2, then `sed -i '/^#!\[allow(clippy::/d' crates/cyrup-session-svc/src/tests/*.rs`. Verify with `cargo clippy -p cyrup-session-svc --all-targets`. One deliberate trade-off to state in the commit: delete_session_file_trash.rs currently withholds `indexing_slicing` and will gain it under the shared waiver — either accept that or re-add a narrower per-file line there.

### Hoist the 46 duplicated `#![allow(clippy::unwrap_used, …)]` lines in `src/tests/` into one line in `src/tests/mod.rs`

`CONFIRMED` · severity **low** · effort **small** · dimension `consistency`

**Evidence.** 45 byte-identical `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` lines in src/tests/ (e.g. src/tests/abort_settles.rs:27, src/tests/round3.rs:5, src/tests/round2.rs:5) plus a 3-lint variant in src/tests/delete_session_file_trash.rs; three files carry none and still compile. The 10 remaining attributes are on inline `mod tests` blocks in src/ and use six different subsets: src/hooks.rs:341 and src/attribution.rs:148 list 2 lints, src/tools.rs:310 and src/host_services.rs:1879 list 3, src/builder.rs:2345 and src/state.rs:351 list 4, and src/bash.rs uses two different subsets at :584 and :714. Verified empirically in this crate that one inner attribute in src/tests/mod.rs suppresses the deny in child modules declared in separate files.

**Why it matters.** 46 lines of boilerplate that must be remembered on every new test file, in a form that has already drifted (three files forgot it, one uses a narrower subset). The six subsets on the inline `mod tests` blocks make it impossible to tell at a glance whether a narrower list was a deliberate tightening or just what the author pasted.

**Fix.** Put one `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]` immediately after the module doc in src/tests/mod.rs and delete the 46 per-file copies. Separately normalise the 10 inline-`mod tests` attributes in src/ to the same 4-lint list, or leave them — but state which.
